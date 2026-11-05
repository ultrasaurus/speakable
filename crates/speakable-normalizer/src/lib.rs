//! Text normalization applied before text-to-speech: rewrites a sentence
//! into the words a voice should actually say ("1976-77" -> "nineteen
//! seventy six to seventy seven", "e.g." -> "for example").
//!
//! Processing order, in outline (the full, numbered pass list is in
//! `normalize_with_spans_in`):
//!   0. Strip short inline quotes (≤5 words): "foo bar" → foo bar.
//!   1. Expand known `<Tag-N>` citation/figure/table markers.
//!   2. Expand year ranges: 1976-77 → "1976 to 77".
//!   2b. Expand comma-grouped numbers: 2,000 → "two thousand".
//!   2c. Spell item/reference numbers digit-by-digit: Item 71279 → Item seven one …
//!   2d. Expand journal links: (AUGMENT,71279,) → Augment seven one two seven nine
//!   3. Apply punctuated overrides from the override table (e.g. "e.g.").
//!   3b. Replace dots between alphanumeric chars with " dot ": 4b.l → 4b dot l.
//!   4. Tokenize on word boundaries:
//!      a. Apply single-word overrides (case-insensitive).
//!      b. Spell out short all-caps (≤3 chars) letter by letter: UIS → U I S.
//!      c. Lowercase long all-caps (>3 chars): AUGMENT → augment.
//!      d. Insert spaces in alphanumeric tokens: 1a → 1 a, 4c2 → 4 c 2.
//!      e. Spell leading-zero digit strings word-by-word: 0609 → zero six zero nine.
//!   5. Replace remaining hyphens with spaces.
//!   6. Strip bracket characters.
//!   7. Replace ellipses with newlines (with punctuation normalisation).
//!
//! Override tables are plain data: this crate reads and writes no files.
//! [`BUILTIN_OVERRIDES`] is the table it ships with; a caller can parse its
//! own with [`parse_overrides`] and layer it over another with
//! [`Normalizer::with_overrides`].
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

mod bible_refs;

// ---------------------------------------------------------------------------
// Override table
// ---------------------------------------------------------------------------

/// The override table built into this crate, in the format
/// [`parse_overrides`] reads: general spoken forms (symbols, months,
/// abbreviations, units, common acronyms) and names said the same way
/// wherever they appear. [`Normalizer::builtin`] and the free functions
/// ([`normalize`], [`normalize_with_spans`], [`spoken_word_count`]) use it.
pub const BUILTIN_OVERRIDES: &str = include_str!("global_overrides.txt");

/// Normalizes text using one override table: the built-in table, a
/// caller's own, or one table layered over another. Cheap to clone (the
/// table is shared).
#[derive(Clone)]
pub struct Normalizer {
    map: Arc<HashMap<String, String>>,
    /// The table's punctuated entries as Pass 1 matches them (see
    /// [`punctuated_keys`]), built once per table.
    punctuated: Arc<Vec<PunctuatedKey>>,
    /// Hash of the table's entries. Two normalizers with the same
    /// fingerprint produce the same output, so it keys caches of that
    /// output.
    fingerprint: u64,
}

impl Normalizer {
    /// A normalizer using exactly `map`, keyed as [`parse_overrides`] keys
    /// it.
    pub fn from_map(map: HashMap<String, String>) -> Self {
        use std::hash::{Hash, Hasher};
        let mut entries: Vec<_> = map.iter().collect();
        entries.sort();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        entries.hash(&mut h);
        let punctuated = Arc::new(punctuated_keys(&map));
        Normalizer { map: Arc::new(map), punctuated, fingerprint: h.finish() }
    }

    /// A normalizer using [`BUILTIN_OVERRIDES`]. Parsed once per process.
    pub fn builtin() -> Self {
        static BUILTIN: OnceLock<Normalizer> = OnceLock::new();
        BUILTIN.get_or_init(|| Normalizer::from_map(parse_overrides(BUILTIN_OVERRIDES))).clone()
    }

    /// This table with `entries` layered over it: an entry in `entries`
    /// wins over one here with the same key. `entries` should come from
    /// [`parse_overrides`], so its keys are folded the same way.
    pub fn with_overrides(&self, entries: HashMap<String, String>) -> Self {
        if entries.is_empty() { return self.clone(); }
        let mut map = (*self.map).clone();
        map.extend(entries);
        Self::from_map(map)
    }

    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// The table's entries, keyed as [`parse_overrides`] keys them.
    pub fn entries(&self) -> &HashMap<String, String> {
        &self.map
    }

    /// See the free function [`normalize`].
    pub fn normalize(&self, text: &str) -> String {
        self.normalize_with_spans(text).text
    }

    /// See the free function [`normalize_with_spans`].
    pub fn normalize_with_spans(&self, text: &str) -> NormalizedText {
        normalize_with_spans_in(text, &self.map, &self.punctuated)
    }

    /// See the free function [`spoken_word_count`].
    pub fn spoken_word_count(&self, text: &str) -> usize {
        self.normalize(text).split_whitespace().count()
    }
}

/// Parse override lines: a tab between key and value, `# ` comments, one
/// entry per line. [`BUILTIN_OVERRIDES`] is in this format.
pub fn parse_overrides(contents: &str) -> HashMap<String, String> {
    contents.lines().filter_map(parse_override_line).collect()
}

/// One line of an overrides file as `(folded key, value)`, or `None` for a
/// comment, a blank line, or a line with no usable entry.
pub fn parse_override_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    // "# " (hash + space) starts a comment, and a bare "#" line (used
    // as a visual separator between comment blocks) is one too; a
    // hash glued directly to a following char ("#x") is a valid key,
    // not a comment.
    if line.is_empty() || line == "#" || line.starts_with("# ") { return None; }
    // Tab is the required delimiter so that keys can contain spaces
    // (e.g. multi-word punctuated keys like "I. INTRODUCTION").
    let mut cols = line.splitn(2, '\t');
    match (cols.next(), cols.next()) {
        (Some(from), Some(to)) => {
            let to = to.trim().split('#').next().unwrap_or("").trim();
            (!to.is_empty()).then(|| (fold_override_key(from), to.to_owned()))
        }
        (Some(from), None) => {
            tracing::warn!("overrides: no tab on line {:?} — entry ignored (use tab to separate key from value)", from);
            None
        }
        _ => None,
    }
}

/// The key an override is stored under. Word-only keys (Pass 10,
/// single-word lookup) are folded to lowercase — Pass 10 looks them up via
/// `token.to_lowercase()`. Punctuated keys (Pass 1) keep their original
/// case instead, so that an uppercase letter in the key (e.g. "No.") can
/// signal a case-sensitive match — see `apply_one_override_chunk`.
pub fn fold_override_key(key: &str) -> String {
    if key.chars().all(|c| c.is_alphanumeric() || c == '\'') {
        key.to_lowercase()
    } else {
        key.to_owned()
    }
}

/// Word count of `normalize(text)`: the number of words actually spoken,
/// which is what drives a sentence's audio duration. Numbers, citations
/// and symbols expand into several spoken words each, so this can run well
/// above `text`'s raw whitespace word count.
///
/// Not memoized: `normalize` is costly (about half a millisecond per
/// sentence in a release build, roughly ten times that in a debug build),
/// so a caller that re-measures the same text should cache the result,
/// keyed by [`Normalizer::fingerprint`] and the text.
///
/// Uses the built-in table; [`Normalizer::spoken_word_count`] uses a
/// given one.
pub fn spoken_word_count(text: &str) -> usize {
    Normalizer::builtin().spoken_word_count(text)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Normalize `text` for TTS pronunciation, discarding the source-span
/// mapping. Most callers want this; use `normalize_with_spans` when you
/// need to map a position in the normalized output back to the original
/// input (e.g. translating forced-alignment word timestamps).
///
/// Uses the built-in table; [`Normalizer::normalize`] uses a given one.
pub fn normalize(text: &str) -> String {
    Normalizer::builtin().normalize(text)
}

/// Normalized text plus a mapping from char ranges in `text` back to char
/// ranges in the original (pre-normalization) input.
pub struct NormalizedText {
    pub text: String,
    spans: Vec<Spanned>,
}

impl NormalizedText {
    /// Map a char range in the normalized `text` back to the char range in
    /// the original input it was derived from. Returns `None` if
    /// `normalized_range` is out of bounds.
    ///
    /// Granularity is per-chunk, not per-char (see the span-mapping
    /// proof-of-concept notes above `Spanned`): if `normalized_range` spans
    /// multiple chunks, the result covers the union of their source
    /// ranges, not a precise char-for-char mapping.
    pub fn source_range(&self, normalized_range: std::ops::Range<usize>) -> Option<std::ops::Range<usize>> {
        if normalized_range.start >= normalized_range.end { return None; }
        let start = source_span_at(&self.spans, normalized_range.start)?;
        let last_offset = normalized_range.end - 1;
        let end = source_span_at(&self.spans, last_offset)?;
        Some(start.start.min(end.start)..start.end.max(end.end))
    }

    /// Each chunk of the normalized output, in order, tagged with whether
    /// it's "raw" — untouched/unexpanded by every pass, same char count as
    /// its source range. A raw chunk is ordinary prose, safe to compare
    /// word-for-word against source text; a non-raw chunk was transformed
    /// by some pass (a number spelled out, an acronym expanded, a tag or
    /// journal link, a stripped quote) and reflects what's actually
    /// spoken, not literally what the source text said — callers wanting
    /// to compare *spoken content* against source text (e.g. checking a
    /// speech-recognition transcript of the audio) should skip non-raw chunks on both sides rather
    /// than trying to match either form, since a transformation can change
    /// both the wording and the word count.
    pub fn chunks(&self) -> impl Iterator<Item = (&str, bool)> {
        self.spans.iter().map(|s| (s.text.as_str(), is_raw(s)))
    }
}

/// Same normalization as `normalize`, but keeps the source-span mapping
/// needed to translate positions in the output back to the original input.
///
/// Every pass below runs as a `*_spanned` chunk-threading variant instead
/// of a flat-string transform — this is the single source of truth for
/// `normalize()`'s behavior; the individual unspanned pass functions
/// (`expand_numeric_ranges`, `tokenize`, etc., used directly by unit tests)
/// are now thin wrappers delegating to these same spanned passes, so there
/// is no separate parallel implementation to drift out of sync.
///
/// Uses the built-in table; [`Normalizer::normalize_with_spans`] uses a
/// given one.
pub fn normalize_with_spans(text: &str) -> NormalizedText {
    Normalizer::builtin().normalize_with_spans(text)
}

/// Passes 0–0f, the passes before Pass 1. Punctuated override keys are run
/// through these too, so a key written the way the source text reads
/// matches the text these passes leave.
fn early_passes_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    // Pass 0: replace URLs with "link" — runs before literally everything
    // else, since a URL's digits/dots/colons/query-string punctuation would
    // otherwise get chewed up piecemeal by every later pass (dots read as
    // "point", digit runs spelled out, etc.), and there's no dictionary
    // lookup available to tell a real word from something like
    // "O2PmXP3Lx8paVS7D" — punting on trying to speak URLs meaningfully.
    let chunks = replace_urls_spanned(chunks);

    // Pass 0b: drop any whitespace directly before a comma, or before
    // "!"/"?"/";"/")"/"]" (SPACE_BEFORE_PUNCT_EARLY) — a stray space there
    // ("Press , 1948", "in Mexico City ( above ),") makes the TTS backend
    // vocalize a strange nonsense syllable, and it's just source-text noise
    // with no meaning to preserve. Runs before everything else so no later
    // pass has to account for that extra whitespace. (The same fix for a
    // period runs later, as Pass 5c — see there for why it can't run this
    // early too.)
    let chunks = strip_space_before_comma_spanned(chunks);

    // Pass 0b2: drop any whitespace directly after an opening "(" or "[" —
    // the mirror image of Pass 0b, so "( above )" ends up "(above)" once
    // both passes have run.
    let chunks = strip_space_after_opening_bracket_spanned(chunks);

    // Pass 0c: normalize curly quotes to their ASCII equivalents. Must run
    // before Pass 1 (punctuated overrides, e.g. the `":"` symbol-name entry)
    // and Pass 2 (short-quote stripping) — both only recognize the ASCII
    // `'`/`"` characters, so a curly quote from the source would silently
    // skip both otherwise.
    let chunks = normalize_curly_quotes_spanned(chunks);

    // Pass 0c2: tighten spacing around a double quote — drop a stray space
    // right after an opening `"` and right before a closing `"`
    // (`'" Hi"'` -> `'"Hi"'`, `'"hello, " he said'` -> `'"hello," he said'`).
    // A closing `"` that ends up directly after `.`/`!`/`?` this way is
    // dropped outright here too, rather than left for Pass 5d to pick up —
    // see `tighten_quote_spacing_spanned` for why Pass 5d's later per-chunk
    // scan can't be relied on for that. Must run after Pass 0c (curly quotes
    // already ASCII) so it only has to recognize one quote form.
    let chunks = tighten_quote_spacing_spanned(chunks);

    // Pass 0d: drop a bare colon outright when it's acting as sentence
    // punctuation — preceded by a letter, followed by whitespace
    // ("Note: something" -> "Note something"). A bare colon reaching the
    // TTS backend as literal punctuation produces a short garbled
    // vocalization ("pew"/"puh"), not a real pause; a comma in its place
    // turned out awkward more often than useful. Scoped narrowly so it
    // doesn't touch a colon between digits (times "3:00", ratios "2:1") —
    // neither side of that shape is a letter. Must run after Pass 0 (URL
    // replacement) so a URL's own colons are already gone by the time this
    // runs.
    let chunks = normalize_bare_colon_spanned(chunks);

    // Pass 0e: drop on-the-hour minutes from a clock time immediately
    // before AM/PM ("9:00 AM" -> "9 AM", "11:00 PM" -> "11 PM") — spelling
    // out "zero zero" for the minutes is technically correct but reads
    // awkwardly next to a plain hour number. Must run before Pass 10
    // (tokenization) — by then "9:00" is already three separate tokens
    // ("9", "00" digit-spelled, and the bare colon), too late to recognize
    // as one clock-time unit.
    let chunks = drop_on_the_hour_minutes_spanned(chunks);

    // Pass 0f: expand a degrees-minutes(-seconds) coordinate group
    // ("42° 15′" -> "forty two degrees fifteen minutes") — anchored on the
    // arcminute/arcsecond prime characters specifically, not the ASCII
    // apostrophe/quote (also used for feet/inches, e.g. "5′ 6″" tall).
    // Must run before Pass 1's generic "°" -> "degrees" override, which
    // would otherwise consume the degree symbol first and destroy the
    // anchor this pass needs to tell a following "′" apart from a
    // foot-mark. Spells its own digits, same reasoning as every other
    // digit-producing pass — once expanded, the chunk is non-raw and
    // invisible to Pass 8.
    expand_degree_minute_second_spanned(chunks)
}

fn normalize_with_spans_in(text: &str, overrides: &HashMap<String, String>, punctuated: &[PunctuatedKey]) -> NormalizedText {
    let chunks = early_passes_spanned(spanned_from_input(text));

    // Pass 1: punctuated overrides run first — before quote-stripping — so
    // keys like `"."` and `"*D"` can match before their quotes are removed.
    // Each key has been through the same Passes 0–0f as the text (see
    // `punctuated_keys`), so it still matches text those passes rewrote.
    let chunks = apply_punctuated_overrides_spanned(chunks, punctuated);

    // Pass 1b: spell "am"/"pm" as "A M"/"P M" after a clock time ("9 am",
    // "5pm", "11:00PM"). Must run after Pass 0e, which leaves "11:00PM" as
    // "11", " ", "PM" in separate chunks, and before Pass 8 spells the
    // hour's digits, which this pass looks back for.
    let chunks = expand_am_pm_after_number_spanned(chunks);

    // Pass 2: strip short inline quotes (≤5 words) — prevents TTS mangling
    // of brief quoted phrases by removing the quotation marks.
    let chunks = strip_short_quotes_spanned(chunks);

    // Pass 3: expand <Tag-N> markers.
    let chunks = expand_tags_spanned(chunks);

    // Pass 3b: expand `[N]` / `[N,M]` footnote markers (e.g. "resolution.[1]"
    // -> "resolution. Footnote one.") into spoken form. Same reference-
    // marker family as Pass 3, just square-bracket/no-tag-name syntax — must
    // run before Pass 6a-c/7 spell the digits and before Pass 11 strips the
    // brackets, for the same "can't reach across an already-expanded chunk"
    // reason as everywhere else in this file.
    let chunks = expand_footnote_markers_spanned(chunks);

    // Pass 3c: expand Bible verse references ("Mt. 5:30" -> "Matthew chapter
    // five verse thirty", "Ps. 40:7-8" -> "Psalm forty verse seven to
    // eight"). Must run before Pass 4 (numeric ranges) and Pass 8 (bare
    // numbers) — both would otherwise independently claim and mis-expand
    // the chapter/verse digits before this pass ever sees the whole
    // "Book chapter:verse[-verse2]" shape as one raw chunk to match against
    // (the same "specific before generic" reasoning as everywhere else in
    // this file). Book name/abbreviation matching lives in `bible_refs`
    // since the table is long.
    let chunks = expand_bible_refs_spanned(chunks);

    // Pass 3d: expand a journal citation's volume/issue/pages prefix
    // ("2007;68(3):343-349" -> "2007, volume sixty eight, issue three,
    // pages 343-349"). Must run before Pass 4 — that pass would expand the
    // page range into a non-raw chunk, hiding it from the lookahead that
    // picks "page" vs "pages" — and before Pass 4c, which would otherwise
    // have to guess that "68:343" isn't a ratio. The page numbers are left
    // raw for Pass 4/8 to spell.
    let chunks = expand_citation_volume_pages_spanned(chunks);

    // Pass 4: expand numeric ranges (N-M or N–M, with optional surrounding
    // spaces) to "N to M" — covers year ranges like "1976-77" and general
    // ranges like "300–3.4" or "4 - 5". Both sides are spelled out here
    // since the expanded chunk is non-raw and invisible to later passes.
    // En-dash (U+2013) is treated identically to ASCII hyphen as a range
    // separator. Leading minus signs ("-2") are not affected since no digit
    // precedes the dash.
    let chunks = expand_numeric_ranges_spanned(chunks);

    // Pass 4b: expand fractions (N/M -> "N over M" or a named fraction word
    // like "one third") — must run before Pass 8 (generic bare-number
    // expansion) claims the numerator/denominator digits individually, the
    // same "specific pass before generic pass" reason as everywhere else in
    // this file.
    let chunks = expand_fractions_spanned(chunks);

    // Pass 4c: expand ratios (N:M -> "N to M", "1,000,000,000,000:1" -> "one
    // trillion to one") — must run before Pass 8 claims each side's digits
    // separately and Pass 10 leaves the bare colon between them. An H:MM
    // clock-time shape ("3:15") is deliberately left alone.
    let chunks = expand_ratios_spanned(chunks);

    // Pass 5: replace em-dashes and double-hyphens with comma so TTS gets a
    // clean pause cue instead of extra whitespace that causes vocalisation
    // artifacts. A leading em-dash at the very start of the sentence (a
    // block-quote attribution like "—William Penn, 1694,") is dropped
    // instead of turned into a comma — see `drop_leading_em_dash`.
    let chunks = replace_em_dash_and_double_hyphen_spanned(chunks);

    // Pass 5b: replace ellipses (`...` or `…`) with a single separating
    // space where one is needed, dropped entirely otherwise. Must run here,
    // before Pass 10's tokenizer — tokenizing splits each non-alphanumeric
    // character (including each `.` in a bare `...`) into its own chunk, so
    // a literal-substring match run afterward can never see the 3-char
    // pattern again; only the single-codepoint `…` form would still match.
    // See `replace_ellipsis_chunk` for the space-insertion rule and the
    // history of prior attempts at this pass.
    let chunks = replace_ellipsis_spanned(chunks);

    // Pass 5c: drop any whitespace directly before a period — same artifact
    // and fix as Pass 0b's comma case ("the end ." -> "the end."), just for
    // a period. Must run after Pass 5b (ellipsis), not alongside Pass 0b —
    // stripping the space before every period that early would split a
    // spaced-out ellipsis (". . . .") into several single-character chunks,
    // too short on their own for Pass 5b's contiguous-run scan to
    // recognize. By this point any spaced ellipsis is already resolved, so
    // every period left is a genuine, isolated one.
    let chunks = strip_space_before_period_spanned(chunks);

    // Pass 5d: drop any run of quote characters (' or ") immediately after
    // terminal punctuation (. ! ?) — e.g. a double-quoted piece of dialogue
    // closing right at the end of a surrounding single-quoted block (`."'`:
    // the "-close for the dialogue, then the '-close for the block), seen
    // right before a leaked-audio tail artifact in a real sample, reaches
    // TTS as literal punctuation. Quotes are never spoken and Pass 2
    // already discards them wholesale rather than preserving
    // balance/nesting, so this extends the same policy to end-of-sentence
    // quote runs instead of trying to keep them paired. Must run after Pass
    // 0c (curly quotes already normalized to ASCII '/") so it only has to
    // recognize one quote form.
    let chunks = strip_trailing_quotes_after_terminal_punctuation_spanned(chunks);

    // Pass 6a: expand a currency amount glued directly to a leading "£" or
    // "$" ("£2333" -> "2333 pounds", "$45.50" -> "forty five dollars and
    // fifty cents"). Must run before Pass 6c (identifier dots) — otherwise
    // the "." in a decimal amount would already be turned into " point "
    // before this pass ever saw the amount as one whole, still-raw "$45.50"
    // shape to match against (the same "can't reach across an
    // already-expanded chunk" trap documented for Pass 4).
    let chunks = expand_currency_spanned(chunks);

    // Pass 6b: replace a bare "#" immediately followed by a digit with
    // "number " (e.g. "#26.44" -> "number 26.44"), the "number sign" usage
    // (as opposed to a hashtag or "C#", which don't have a digit right
    // after the "#" and are left untouched). Must run before Pass 6d/8
    // spell those digits out, for the same reason as Pass 6c below — a
    // bare "#" glued to an already-expanded digit chunk would be stranded
    // with no adjacent raw text left to match against.
    let chunks = replace_number_sign_spanned(chunks);

    // Pass 6c: replace dots between alphanumeric chars with " dot " (link
    // notation like `4b.l` or `Ref.dt`), or " point " when digits flank
    // both sides (`26.44` -> "twenty six point forty four"). Must run
    // before Pass 6d/8 — those passes spell the digit runs on either side
    // of the dot independently and mark each resulting chunk non-raw,
    // which would strand the dot as an isolated 1-char raw chunk with no
    // adjacent text left for this pass to see (the same "can't reach
    // across an already-expanded chunk" trap documented for Pass 4).
    // Running first also means the point-vs-dot decision can inspect the
    // still-raw digits directly, no extra bookkeeping needed.
    let chunks = replace_identifier_dots_spanned(chunks);

    // Pass 6d: spell Item/reference numbers digit-by-digit (Item 71279 →
    // Item seven one two seven nine) so TTS doesn't garble large IDs. Runs
    // before the generic bare-number rule (Pass 8) — Item/Ref numbers are
    // an ID-style exception (digit-by-digit) that the generic digit-group
    // rule would otherwise shadow for 1-4 digit Item/Ref numbers (e.g.
    // "Item 1000" must stay "Item one zero zero zero", not "Item one
    // thousand"). Once this pass expands a match, the chunk becomes
    // non-raw, so Pass 8 correctly skips re-processing it.
    let chunks = spell_item_numbers_spanned(chunks);

    // Pass 6e: expand a cent-sign amount glued directly to a digit run
    // ("1¢" -> "one cent", "3¢" -> "three cents") — singular/plural
    // agreement same as Pass 4b's fraction numerator, digit spelling via
    // the same `spell_bare_digit_group` helper Pass 4/6d/7/8 all share. A
    // bare, non-digit-adjacent "¢" (e.g. "denoted ¢") has no number to
    // agree with and expands to plain "cents" instead. Handled as its own
    // pass rather than an override table entry, since a plain-text
    // override can't tell the two cases apart to pick the right
    // singular/plural form or spell the preceding digit out. Runs before
    // Pass 8 for the same "specific before generic" reason as Pass 6d/7,
    // so the claimed digit run doesn't get spelled out again with no
    // "cent(s)" suffix.
    let chunks = expand_cent_sign_spanned(chunks);

    // Pass 6f: expand an ordinal number — a digit run immediately followed
    // by "st"/"nd"/"rd"/"th" ("15th" -> "fifteenth", "1st" -> "first") — so
    // the suffix doesn't reach Pass 10's `split_alphanumeric` and get left
    // stranded as raw, unspoken letters after the digit run is spelled out
    // ("15th" -> "fifteen th" otherwise). Runs before Pass 8 for the same
    // "specific before generic" reason as Pass 6d/6e/7.
    let chunks = expand_ordinals_spanned(chunks);

    // Pass 7: expand journal links like (AUGMENT,71279,) or <OAD,2237,> →
    // "Augment 7 1 2 7 9". Runs before bracket-stripping so the delimiters
    // are consumed as part of the pattern. Also must run before Pass 8 (the
    // generic bare-number rule) — since Pass 8 grew a fallback for bare 5+
    // digit numbers, a journal link's digit run (often 5 digits, e.g.
    // "81010") would otherwise get claimed and spelled out by Pass 8 before
    // this pass ever sees the whole "(AUGMENT, 81010,)" shape as one raw
    // chunk to match against.
    let chunks = expand_journal_links_spanned(chunks);

    // Pass 8: expand bare numbers not already handled above — comma-grouped
    // ("2,000" -> "two thousand") or digit-group style for 1-4 digit runs
    // ("560" -> "five sixty", "1976" -> "nineteen seventy six"), plus a
    // digit-by-digit fallback for bare 5+ digit runs ("87574" -> "eight
    // seven five seven four") that the digit-group convention doesn't cover.
    let chunks = expand_comma_numbers_spanned(chunks);

    // Pass 8b: expand a rate-unit slash ("cycles/sec" -> "cycles per
    // second", "bands/decade" -> "bands per decade") — only when the word
    // right after the slash is a known rate unit, so ordinary word/word
    // pairs ("and/or", "his/her", "document/file") are left untouched.
    let chunks = expand_rate_units_spanned(chunks);

    // Pass 9: expand US state postal abbreviations in "City, ST" position
    // (e.g. "Denver, CO" → "Denver, Colorado"). Only fires on the comma-led
    // pattern, not as a standalone word override, because many codes (IN, OR,
    // ME, HI, OK, OH, ...) collide with common English words — a flat
    // word-list override would mangle those everywhere they appear.
    let chunks = expand_state_abbrevs_spanned(chunks);

    // Pass 10: tokenize and process word/alphanumeric tokens (override
    // lookup, alphanumeric splitting, all-caps handling, Roman numerals,
    // leading-zero digit sequences); replace remaining hyphens with spaces.
    let chunks = tokenize_spanned(chunks, &overrides);

    // Pass 11: strip bracket characters (keeping their contents) — VibeVoice
    // hallucinates on tokens with `<>`/`()`/`[]` next to other punctuation.
    let chunks = strip_brackets_spanned(chunks);

    // Pass 11b: drop bare `|` tokens (table-cell separators and byline
    // dividers left in plain text) — never spoken and artifact-free, but
    // each one would otherwise count as a word in `spoken_word_count` and
    // in any word-count limit a caller applies to the normalized text.
    let chunks = strip_bare_pipes_spanned(chunks);

    // Pass 12: capitalize the sentence-opening character if it's lowercase.
    // Unconditional — regardless of whether it's original source text or a
    // pass's output (e.g. a single-word override like "sql" -> "sequel"
    // used verbatim) — since this text is only ever heard via TTS, never
    // shown to a reader, so there's no orthographic reason to preserve a
    // deliberately-lowercase original spelling (e.g. "iPhone") here; only
    // pronunciation matters, and a lowercase word opening a sentence has
    // been observed to cause TTS artifacts.
    let chunks = capitalize_sentence_start_spanned(chunks);

    let text = flatten(&chunks);
    NormalizedText { text, spans: chunks }
}

// ---------------------------------------------------------------------------
// Pass 3: <Tag-N> expansion
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_tags(text: &str) -> String {
    flatten(&expand_tags_spanned(spanned_from_input(text)))
}

fn try_expand_tag(inner: &str) -> Option<String> {
    let (word, digits) = inner.split_once('-')?;
    if !word.chars().next()?.is_uppercase() { return None; }
    if !digits.chars().all(|c| c.is_ascii_digit()) { return None; }

    let spoken = match word.to_lowercase().as_str() {
        "fig"   => "figure".to_owned(),
        "table" => "table".to_owned(),
        other   => other.to_owned(),
    };
    let spoken = {
        let mut c = spoken.chars();
        match c.next() {
            None => String::new(),
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        }
    };
    // Spell the digits out now, the same way Pass 4 (numeric ranges) does —
    // once this tag is expanded the chunk is no longer raw, so Pass 8's
    // bare-number scan can never reach these digits afterward (chunk-
    // granularity design). Left bare, they'd be invisible to forced
    // alignment entirely, not just unspoken awkwardly. Falls back to the
    // bare digits for 5+ digit tags, which `spell_bare_digit_group` doesn't
    // cover.
    let spoken_digits = spell_bare_digit_group(digits).unwrap_or_else(|| digits.to_owned());
    Some(format!("{} {}", spoken, spoken_digits))
}

/// Span-aware version of `expand_tags`. When a `<...>` doesn't expand (no
/// closing `>`, or `try_expand_tag` rejects it), the re-emitted text is
/// identical to the input for that range, so it's simply left in the
/// pending unprocessed run rather than flushed as a no-op replacement.
fn expand_tags_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_tags_chunk(&chunk)).collect()
}

fn expand_tags_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if chars[i] == '<' {
            let start = i;
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '>' && chars[j] != '<' { j += 1; }
            if j < chars.len() && chars[j] == '>' {
                let tag: String = chars[start + 1..j].iter().collect();
                let end = j + 1;
                if let Some(expanded) = try_expand_tag(&tag) {
                    flush(&mut out, &chars, base, run_start, start);
                    out.push(Spanned { text: expanded, src: (base + start)..(base + end) });
                    run_start = end;
                    i = end;
                    continue;
                }
                // no expansion — re-emitted text equals the input slice, no-op.
                i = end;
                continue;
            }
            // no closing '>' — also a no-op for this range.
            i = j;
            continue;
        }
        i += 1;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 3b: footnote markers ([N], [N,M], [N, M], [N–M])
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_footnote_markers(text: &str) -> String {
    flatten(&expand_footnote_markers_spanned(spanned_from_input(text)))
}

/// If `chars[start]` begins a footnote marker for the given `closer`
/// (`']'` for a `[`-opened marker, `'*'` for an asterisk-delimited one) —
/// one or more comma-separated (optionally comma-space-separated) digit
/// runs, closed by `closer` — returns the index just past `closer` and the
/// digit runs found. `None` if it's some other use of the opener (e.g.
/// `[a]`, unterminated `[1`, or `*emphasis*` with non-digit content).
///
/// A `^` immediately after `start` is skipped first — Markdown
/// footnote-reference syntax (`[^3]`) is the same marker convention with
/// an extra sigil, not a different shape, so `[^3]` reads exactly like
/// `[3]` ("Footnote three.") rather than falling through unrecognized.
///
/// An item can also be a range, two digit runs joined by an en dash or a
/// hyphen (`[4–8]`, `[1, 4–8]`), as a journal article's citation
/// superscripts are written once converted to plain text.
fn try_parse_footnote_marker(chars: &[char], start: usize, closer: char) -> Option<(usize, Vec<FootnoteItem>)> {
    fn digit_run(chars: &[char], mut j: usize) -> (usize, String) {
        let digit_start = j;
        while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
        (j, chars[digit_start..j].iter().collect())
    }

    let mut j = start + 1;
    if chars.get(j) == Some(&'^') { j += 1; }
    let mut numbers = Vec::new();
    loop {
        let (after, first) = digit_run(chars, j);
        if first.is_empty() { return None; }
        j = after;
        let mut last = None;
        if matches!(chars.get(j), Some('–' | '-')) {
            let (after, end) = digit_run(chars, j + 1);
            if end.is_empty() { return None; }
            j = after;
            last = Some(end);
        }
        numbers.push(FootnoteItem { first, last });
        if j < chars.len() && chars[j] == ',' {
            j += 1;
            if j < chars.len() && chars[j] == ' ' { j += 1; }
            continue;
        }
        break;
    }
    if j < chars.len() && chars[j] == closer { Some((j + 1, numbers)) } else { None }
}

/// One item of a footnote marker: a number, or a range `first–last`.
struct FootnoteItem {
    first: String,
    last: Option<String>,
}

/// Spells each footnote number (falling back to the raw digits for the rare
/// 5+ digit case `spell_bare_digit_group` doesn't cover) and joins them the
/// way a reader would say a citation list aloud: singular for one, "...and
/// ..." for two, an Oxford-comma list for 3+. A range reads "four to
/// eight" and always takes the plural. Returns `("Footnote"/"Footnotes",
/// joined number words)` — unpunctuated and uncapitalized, so callers can
/// build either a standalone sentence (`format_footnote_words`) or a
/// lowercase mid-sentence aside (`format_footnote_aside`) from the same
/// underlying word list.
fn footnote_label(numbers: &[FootnoteItem]) -> (&'static str, String) {
    let spell = |n: &String| spell_bare_digit_group(n).unwrap_or_else(|| n.clone());
    let words: Vec<String> = numbers.iter()
        .map(|item| match &item.last {
            Some(last) => format!("{} to {}", spell(&item.first), spell(last)),
            None => spell(&item.first),
        })
        .collect();
    match words.as_slice() {
        [one] if numbers[0].last.is_some() => ("Footnotes", one.clone()),
        [one] => ("Footnote", one.clone()),
        [a, b] => ("Footnotes", format!("{a} and {b}")),
        [rest @ .., last] => ("Footnotes", format!("{}, and {last}", rest.join(", "))),
        [] => unreachable!("try_parse_footnote_marker always finds at least one digit run"),
    }
}

/// `"Footnote three."` / `"Footnotes two and three."` — a standalone,
/// capitalized, period-terminated clause. Used when the marker sits at an
/// actual sentence boundary. No leading space — the caller prepends the
/// right separator.
fn format_footnote_words(numbers: &[FootnoteItem]) -> String {
    let (label, body) = footnote_label(numbers);
    format!("{label} {body}.")
}

/// `"footnote three"` / `"footnotes two and three"` — lowercase, no
/// punctuation at all. Used when the marker sits mid-sentence, where it
/// reads as a comma-set-off aside rather than a fake new sentence; the
/// caller supplies whatever comma(s) belong around it.
fn format_footnote_aside(numbers: &[FootnoteItem]) -> String {
    let (label, body) = footnote_label(numbers);
    format!("{} {body}", label.to_lowercase())
}

/// Span-aware version. A footnote marker glued directly onto preceding text
/// with no space (`"resolution.[1]"`, `"Woodbrooke[3]"`, or the
/// asterisk-delimited `"resolution.*1*"` — all real conventions source
/// documents use) is the case that most needs fixing — bracket-stripping
/// (Pass 11) would otherwise delete the `[`/`]` with nothing put in their
/// place, leaving the spelled-out digit glued straight onto the previous
/// word (`"resolution.one"`). A preceding space, if present, is consumed
/// as part of the match — the replacement always supplies its own
/// separator — so the output is identical either way.
///
/// Four cases, decided by what surrounds the marker:
/// - **Already at a sentence boundary** (preceded by `.`/`!`/`?`/`…`, e.g.
///   `"resolution."`) — unchanged from the original design: a standalone
///   `" Footnote N."` clause.
/// - **Immediately followed by sentence-terminal punctuation** (no space,
///   e.g. `"another[^4]."`) — the marker sits at the *tail* of the current
///   sentence, whose own punctuation is right there; inserting another
///   would double it (`"Footnote four.."`). Reads as `", footnote four"`,
///   leaving the source's own `.` to close the sentence once.
/// - **Followed by lowercase prose** (e.g. `"test [^3] and another"`) —
///   genuinely mid-sentence; the old design forced `". Footnote three."`
///   here, fracturing one sentence into fragments with a stray lowercase
///   `"and"`. Reads as a comma-set-off aside instead: `", footnote three,"`.
/// - **Anything else** (uppercase following, end of text, or other
///   punctuation) — no reliable signal either way; falls back to the
///   original standalone-sentence style, same as the boundary case.
///
/// A marker with nothing but whitespace before it in the whole input isn't
/// a footnote at all — nothing precedes it to cite. It's a list number
/// (`"[49] Utley 2012; ..."` in a numbered bibliography) and reads like the
/// `"49."` list style: `"Forty nine."`. A multi-number marker there
/// (`"[2,3] ..."`) isn't a plausible list number, so it keeps the footnote
/// reading, minus the leading `". "` separator.
fn expand_footnote_markers_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    // An earlier pass may have split the input so a marker sits at a chunk
    // edge, with the punctuation that decides its reading (`"resolution."`
    // before it, `"."` after it) in the neighboring chunk. So each chunk is
    // judged with the surrounding text's edges in view, and whether it
    // starts the whole text.
    const CONTEXT: usize = 16;
    let mut at_text_start = true;
    let mut prev_tail: Vec<char> = Vec::new();
    let mut out = Vec::new();
    for (idx, chunk) in chunks.iter().enumerate() {
        let next_head: Vec<char> = chunks.get(idx + 1)
            .map(|next| next.text.chars().take(CONTEXT).collect())
            .unwrap_or_default();
        out.extend(expand_footnote_markers_chunk(chunk, at_text_start, &prev_tail, &next_head));
        at_text_start = at_text_start && chunk.text.trim().is_empty();
        prev_tail.extend(chunk.text.chars());
        let excess = prev_tail.len().saturating_sub(CONTEXT);
        prev_tail.drain(..excess);
    }
    out
}

fn expand_footnote_markers_chunk(
    chunk: &Spanned,
    at_text_start: bool,
    prev_tail: &[char],
    next_head: &[char],
) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    // The chunk plus its neighbors' edges, for looking around a marker;
    // `off` maps a chunk index into it.
    let ctx: Vec<char> = prev_tail.iter().chain(&chars).chain(next_head).copied().collect();
    let off = prev_tail.len();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        let closer = match chars[i] {
            '[' => Some(']'),
            '*' => Some('*'),
            _ => None,
        };
        if let Some(closer) = closer {
            if let Some((end, numbers)) = try_parse_footnote_marker(&chars, i, closer) {
                let has_space_before = i > 0 && chars[i - 1].is_whitespace();
                let leads_text = at_text_start && chars[..i].iter().all(|c| c.is_whitespace());
                // Leading the text, all the whitespace before it is dropped
                // too, so the list number starts the output.
                let match_start = if leads_text { 0 } else if has_space_before { i - 1 } else { i };

                let (ci, cend) = (i + off, end + off);
                let space_before = ci > 0 && ctx[ci - 1].is_whitespace();
                let prev_idx = if space_before { ci.checked_sub(2) } else { ci.checked_sub(1) };
                // A closing quote or parenthesis can sit between the
                // sentence's terminal punctuation and the marker
                // (`Meeting.”[32]`, curly quote already ASCII-folded to `"`
                // by Pass 0c; likewise `'` from `’`, or `)` closing a
                // citation's parenthetical) — skip over those to find the
                // punctuation they're closing.
                let mut skip_idx = prev_idx;
                while ctx.get(skip_idx.unwrap_or(usize::MAX)).is_some_and(|c| matches!(c, '"' | '\'' | ')')) {
                    skip_idx = skip_idx.and_then(|idx| idx.checked_sub(1));
                }
                let terminal_before = skip_idx
                    .and_then(|idx| ctx.get(idx))
                    .is_some_and(|c| matches!(c, '.' | '!' | '?' | '\u{2026}'));

                // A clause separator right after the marker (`settings [1, 2],
                // empirical...`) reads it as an aside too, the same as a
                // terminator does: `", footnotes one and two"`, then the
                // source's own separator.
                let terminal_after = ctx.get(cend).is_some_and(|c| matches!(c, '.' | '!' | '?' | ',' | ';' | ':'));
                let after_ws = { let mut k = cend; while ctx.get(k) == Some(&' ') { k += 1; } k };
                let lowercase_continuation = !terminal_after
                    && ctx.get(after_ws).is_some_and(|c| c.is_lowercase());

                let replacement = if leads_text {
                    match numbers.as_slice() {
                        [FootnoteItem { first: n, last: None }] => {
                            let words = spell_bare_digit_group(n).unwrap_or_else(|| n.clone());
                            let mut cs = words.chars();
                            let first = cs.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
                            format!("{first}{}.", cs.as_str())
                        }
                        _ => format_footnote_words(&numbers),
                    }
                } else if terminal_before {
                    format!(" {}", format_footnote_words(&numbers))
                } else if terminal_after {
                    format!(", {}", format_footnote_aside(&numbers))
                } else if lowercase_continuation {
                    format!(", {},", format_footnote_aside(&numbers))
                } else {
                    format!(". {}", format_footnote_words(&numbers))
                };

                flush(&mut out, &chars, base, run_start, match_start);
                out.push(Spanned {
                    text: replacement,
                    src: (base + match_start)..(base + end),
                });
                run_start = end;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 3c: Bible verse references ("Mt. 5:30", "Ps. 40:7-8")
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_bible_refs(text: &str) -> String {
    flatten(&expand_bible_refs_spanned(spanned_from_input(text)))
}

/// Reads a digit run starting at `i`, returning its end index (`i` itself
/// if no digits are there).
fn read_digits(chars: &[char], i: usize) -> usize {
    let mut j = i;
    while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
    j
}

fn spell_digits(digits: &str) -> String {
    spell_bare_digit_group(digits).unwrap_or_else(|| {
        digits.chars().map(digit_word).collect::<Vec<_>>().join(" ")
    })
}

/// If a Bible verse reference starts at `chars[i]` — a known book name or
/// abbreviation, whitespace, a chapter digit run, `:`, a verse digit run,
/// and optionally `-` plus a same-chapter verse2 digit run — returns the
/// spoken replacement and how many chars the whole reference consumed.
/// Cross-chapter ranges ("John 3:16-4:2") aren't recognized here; only the
/// chapter:verse[-verse2] shape is.
fn match_bible_ref(chars: &[char], i: usize) -> Option<(String, usize)> {
    let (book, psalms_style, book_len) = bible_refs::match_book(chars, i)?;

    let mut j = i + book_len;
    let ws_start = j;
    while j < chars.len() && chars[j] == ' ' { j += 1; }
    if j == ws_start { return None; }

    let chapter_start = j;
    let chapter_end = read_digits(chars, chapter_start);
    if chapter_end == chapter_start { return None; }

    if chapter_end >= chars.len() || chars[chapter_end] != ':' { return None; }
    let verse_start = chapter_end + 1;
    let verse_end = read_digits(chars, verse_start);
    if verse_end == verse_start { return None; }

    let mut end = verse_end;
    let mut verse2: Option<(usize, usize)> = None;
    if end < chars.len() && chars[end] == '-' {
        let v2_start = end + 1;
        let v2_end = read_digits(chars, v2_start);
        if v2_end > v2_start {
            verse2 = Some((v2_start, v2_end));
            end = v2_end;
        }
    }

    let chapter: String = chars[chapter_start..chapter_end].iter().collect();
    let verse: String = chars[verse_start..verse_end].iter().collect();

    let mut spoken = String::new();
    spoken.push_str(book);
    spoken.push(' ');
    if !psalms_style {
        spoken.push_str("chapter ");
    }
    spoken.push_str(&spell_digits(&chapter));
    spoken.push_str(" verse ");
    spoken.push_str(&spell_digits(&verse));
    if let Some((v2_start, v2_end)) = verse2 {
        let verse2_digits: String = chars[v2_start..v2_end].iter().collect();
        spoken.push_str(" to ");
        spoken.push_str(&spell_digits(&verse2_digits));
    }

    Some((spoken, end - i))
}

fn expand_bible_refs_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_bible_refs_chunk(&chunk)).collect()
}

fn expand_bible_refs_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if let Some((spoken, len)) = match_bible_ref(&chars, i) {
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned { text: spoken, src: (base + i)..(base + i + len) });
            run_start = i + len;
            i += len;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 4: numeric range expansion
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_numeric_ranges(text: &str) -> String {
    flatten(&expand_numeric_ranges_spanned(spanned_from_input(text)))
}

// ---------------------------------------------------------------------------
// Span-mapping proof of concept (not yet wired into `normalize()`).
//
// Goal: map a char offset in normalized output text back to the char range
// in the original input it was derived from — needed so a caller that
// normalizes before synthesis can translate forced-alignment word
// timestamps, which land on normalized-text offsets, back onto the original
// sentence text (e.g. to find the audio for a passage a reader selected).
//
// Approach: thread `Spanned` chunks through each pass instead of a flat
// `String`. Each chunk's `src` range always refers to char offsets in the
// *original* input, never the previous pass's output, so spans compose
// correctly across multiple passes without re-deriving anything.
//
// Granularity is per-chunk, not per-char: an expanded chunk (e.g. "1976 to
// 77") maps as a whole back to its source range (e.g. "1976-77"); we don't
// try to say which output character corresponds to which input character
// within an expansion. That's coarser than per-char tracking but sufficient
// for word-boundary lookups (finding a selected passage's words), and much
// simpler to get right across many transform rules.
//
// Not yet called from `normalize()` — only 2 of ~13 passes are converted so
// far (proof of concept). Exercised by tests below; `#[allow(dead_code)]`
// until the remaining passes are converted and this gets wired in.
// ---------------------------------------------------------------------------

/// A run of normalized text tagged with the char-index range in the
/// original (pre-normalization) input it was derived from.
#[derive(Debug, Clone, PartialEq)]
struct Spanned {
    text: String,
    src: std::ops::Range<usize>,
}

/// Wrap the whole input as a single unprocessed chunk, ready for the first pass.
fn spanned_from_input(text: &str) -> Vec<Spanned> {
    vec![Spanned { text: text.to_owned(), src: 0..text.chars().count() }]
}

/// True if `chunk`'s text is still in 1:1 char-length correspondence with
/// its source range — i.e. no earlier pass has expanded/contracted it yet.
///
/// Scan-and-subdivide passes (the "flush + expand" ones, plus tokenize and
/// punctuated-overrides) must treat a non-1:1 chunk as opaque/atomic rather
/// than re-scanning into it: once a chunk's text has grown or shrunk from
/// its original source range, local char offsets within that text no
/// longer correspond to offsets in the original source, so computing
/// `base + local_offset` would silently produce wrong spans. This also
/// matches the chunk-granularity design intent — text produced by an
/// earlier expansion (e.g. "seven one two") is already finalized spoken
/// form and shouldn't be re-split and re-matched by later passes.
fn is_raw(chunk: &Spanned) -> bool {
    chunk.text.chars().count() == chunk.src.len()
}

/// Concatenate chunks back into a flat string — equivalent to what the
/// unspanned pass would have returned.
fn flatten(chunks: &[Spanned]) -> String {
    chunks.iter().map(|c| c.text.as_str()).collect()
}

/// Map a char offset into the flattened/normalized text to the source char
/// range of the chunk it falls in, or `None` if out of bounds.
fn source_span_at(chunks: &[Spanned], normalized_offset: usize) -> Option<std::ops::Range<usize>> {
    let mut pos = 0;
    for chunk in chunks {
        let chunk_len = chunk.text.chars().count();
        if normalized_offset < pos + chunk_len {
            return Some(chunk.src.clone());
        }
        pos += chunk_len;
    }
    None
}

/// Scans a number (integer, comma-grouped integer like "12,000", or decimal
/// like "3.4") starting at position j. Returns (end_position, matched_text)
/// or None. Shared by Pass 4 (numeric ranges) and Pass 4c (ratios).
fn scan_number(chars: &[char], mut j: usize) -> Option<(usize, String)> {
    if j >= chars.len() || !chars[j].is_ascii_digit() { return None; }
    let start = j;
    while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
    // Optional comma groups: ",ddd" not followed by another digit — the
    // same grouping rule Pass 8 uses. Without this, "100-12,000" matched
    // as the range "100-12", leaving ",000" to be read digit by digit.
    let int_end = j;
    while j + 3 < chars.len() && chars[j] == ','
        && chars[j + 1..j + 4].iter().all(|c| c.is_ascii_digit())
        && (j + 4 == chars.len() || !chars[j + 4].is_ascii_digit())
    {
        j += 4;
    }
    if j > int_end { return Some((j, chars[start..j].iter().collect())); }
    // Optional decimal part: ".digits"
    if j + 1 < chars.len() && chars[j] == '.' && chars[j + 1].is_ascii_digit() {
        j += 1; // consume '.'
        while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
    }
    Some((j, chars[start..j].iter().collect()))
}

/// Spells an integer-or-decimal number as words. Callers that expand a
/// match into a non-raw chunk must spell their digits here — the chunk is
/// invisible to Pass 6c/7.
fn spell_number(s: &str) -> String {
    // A comma-grouped number reads as a magnitude ("twelve thousand"),
    // not in the digit-group style.
    if s.contains(',') {
        let digits: String = s.chars().filter(|c| *c != ',').collect();
        return match digits.parse::<u64>() {
            Ok(n) => number_to_words(n),
            Err(_) => spell_digit_run(&digits),
        };
    }
    match s.split_once('.') {
        Some((int_part, dec_part)) => {
            format!("{} point {}", spell_digit_run(int_part), spell_digit_run(dec_part))
        }
        None => spell_digit_run(s),
    }
}

/// A number whose integer part has a leading zero (length > 1, e.g.
/// "061844") reads as an ID/serial number, not a magnitude — same
/// "leading zero signals an ID" convention Pass 8/Pass 10 already use
/// for bare numbers elsewhere in this file.
fn looks_like_id(num: &str) -> bool {
    let int_part = num.split('.').next().unwrap_or(num);
    int_part.len() > 1 && int_part.starts_with('0')
}

/// Span-aware version of `expand_numeric_ranges`.
fn expand_numeric_ranges_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_numeric_ranges_chunk(&chunk)).collect()
}

fn expand_numeric_ranges_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    // Scans a maximal chain of `number(-number)*` / `number(–number)*`
    // segments starting at `start` (already known to be a valid digit-run
    // start, not preceded by a letter). Returns the end position of the
    // whole chain and the list of number segments found. Each leg reuses
    // the same spacing rule as a standalone pair (see below) — a leg whose
    // spacing doesn't qualify, or that isn't followed by another number,
    // ends the chain there rather than being consumed.
    fn scan_number_chain(chars: &[char], start: usize) -> (usize, Vec<String>) {
        let mut segments = Vec::new();
        let Some((mut pos, first)) = scan_number(chars, start) else { return (start, segments) };
        segments.push(first);
        loop {
            let mut j = pos;
            let space_before = j < chars.len() && chars[j] == ' ';
            if space_before { j += 1; }
            let Some(&separator) = chars.get(j).filter(|c| matches!(c, '-' | '\u{2010}' | '\u{2011}' | '\u{2013}')) else { break };
            j += 1;
            let space_after = j < chars.len() && chars[j] == ' ';
            // En-dash is typographically unambiguous as a range marker, so
            // either side may be spaced independently ("300–3.4",
            // "300 – 3.4"). An ASCII hyphen is genuinely ambiguous with a
            // minus sign, so it only counts as a separator when spacing is
            // symmetric — both sides spaced ("pages 4 - 5") or neither
            // ("1976-77") — never just one side, since "10 -2" (ten,
            // followed by a spaced-out negative two) must not be read as
            // "ten to two".
            if separator != '\u{2013}' && space_before != space_after { break; }
            if space_after { j += 1; }
            let Some((after_num, num)) = scan_number(chars, j) else { break };
            segments.push(num);
            pos = after_num;
        }
        (pos, segments)
    }

    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let digit_start = i;
            // A letter directly before the digit run means this is part of
            // a larger alphanumeric identifier (e.g. "F30602-76-C-003") —
            // leave it for Pass 10's split_alphanumeric instead.
            let preceded_by_letter = digit_start > 0 && chars[digit_start - 1].is_alphabetic();
            if !preceded_by_letter {
                let (chain_end, segments) = scan_number_chain(&chars, digit_start);
                // Exactly two segments and a genuine range shape: expand.
                // Three or more segments (e.g. an ISBN "0-87574-297-3" or a
                // catalog number "AB-12-34-5") is an identifier chain, not
                // a range — skip the *whole* chain untouched rather than
                // risking a sub-pair within it (e.g. its last two segments)
                // getting matched as a bogus range on a later iteration.
                // Same for a leading-zero half ("91-061844") — that reads
                // as an ID, not a magnitude (see `looks_like_id`).
                if segments.len() == 2 && !segments.iter().any(|s| looks_like_id(s)) {
                    // Guard: not immediately followed by a letter — avoids
                    // matching identifiers like "F30602-76-C".
                    let followed_by_letter = chain_end < chars.len()
                        && chars[chain_end].is_alphabetic();
                    if !followed_by_letter {
                        flush(&mut out, &chars, base, run_start, digit_start);
                        // Spell out both halves — once expanded the chunk
                        // is non-raw, so Passes 6b/7 can't reach the digits.
                        let expanded = format!(
                            "{} to {}",
                            spell_number(&segments[0]),
                            spell_number(&segments[1]),
                        );
                        out.push(Spanned {
                            text: expanded,
                            src: (base + digit_start)..(base + chain_end),
                        });
                        run_start = chain_end;
                        i = chain_end;
                        continue;
                    }
                } else if segments.len() >= 3 {
                    i = chain_end;
                    continue;
                }
            }
            // Not a range — advance past the digit run.
            while i < chars.len() && chars[i].is_ascii_digit() { i += 1; }
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 4b: fraction expansion
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_fractions(text: &str) -> String {
    flatten(&expand_fractions_spanned(spanned_from_input(text)))
}

/// Denominators with an irregular or named word form, singular and plural
/// (`(2, "half", "halves")`, `(3, "third", "thirds")`, ...). Covers halves
/// through sixteenths — common fractions plus the imperial-measurement
/// range (`5/16 inch`, `3/8 inch`); anything outside this list falls back
/// to a plain "N over M" cardinal reading (`spell_fraction`).
const FRACTION_DENOMINATOR_WORDS: &[(u64, &str, &str)] = &[
    (2, "half", "halves"),
    (3, "third", "thirds"),
    (4, "fourth", "fourths"),
    (5, "fifth", "fifths"),
    (6, "sixth", "sixths"),
    (7, "seventh", "sevenths"),
    (8, "eighth", "eighths"),
    (9, "ninth", "ninths"),
    (10, "tenth", "tenths"),
    (11, "eleventh", "elevenths"),
    (12, "twelfth", "twelfths"),
    (13, "thirteenth", "thirteenths"),
    (14, "fourteenth", "fourteenths"),
    (15, "fifteenth", "fifteenths"),
    (16, "sixteenth", "sixteenths"),
];

/// Spell a `numerator/denominator` pair as words. The numerator is always a
/// plain cardinal (`number_to_words`, not the digit-group style Pass 8 uses
/// for standalone bare numbers — a fraction's numerator reads as a
/// magnitude, not an ID). A denominator in `FRACTION_DENOMINATOR_WORDS`
/// uses its named word, singular for a numerator of 1 (`"1/2"` -> `"one
/// half"`) and plural otherwise (`"2/3"` -> `"two thirds"`); any other
/// denominator falls back to `"N over M"` (`"5/37"` -> `"five over thirty
/// seven"`).
fn spell_fraction(numerator: &str, denominator: &str) -> String {
    let num_word = number_to_words(numerator.parse().unwrap_or(0));
    let denom_n: u64 = denominator.parse().unwrap_or(0);
    match FRACTION_DENOMINATOR_WORDS.iter().find(|&&(d, _, _)| d == denom_n) {
        Some(&(_, singular, plural)) => {
            let denom_word = if numerator == "1" { singular } else { plural };
            format!("{num_word} {denom_word}")
        }
        None => format!("{num_word} over {}", number_to_words(denom_n)),
    }
}

/// Span-aware version of `expand_fractions`.
fn expand_fractions_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_fractions_chunk(&chunk)).collect()
}

fn expand_fractions_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let digit_start = i;
            // A letter directly before the digit run means this is part of
            // a larger alphanumeric identifier, not a fraction — same guard
            // as Pass 4 (numeric ranges).
            let preceded_by_letter = digit_start > 0 && chars[digit_start - 1].is_alphabetic();
            let mut matched = false;
            if !preceded_by_letter {
                let mut j = digit_start;
                while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
                if j < chars.len() && chars[j] == '/' {
                    let denom_start = j + 1;
                    let mut k = denom_start;
                    while k < chars.len() && chars[k].is_ascii_digit() { k += 1; }
                    // Guard: denominator not immediately followed by a
                    // letter — avoids matching an identifier/version shape
                    // like "3/4G" or "5/8ths". A hyphen right after (the
                    // motivating "1/3-octave" shape) is fine — Pass 10's
                    // hyphen-to-space rule handles what follows it, same as
                    // any other bare hyphen.
                    let followed_by_letter = k < chars.len() && chars[k].is_alphabetic();
                    if k > denom_start && !followed_by_letter {
                        let numerator: String = chars[digit_start..j].iter().collect();
                        let denominator: String = chars[denom_start..k].iter().collect();
                        flush(&mut out, &chars, base, run_start, digit_start);
                        out.push(Spanned {
                            text: spell_fraction(&numerator, &denominator),
                            src: (base + digit_start)..(base + k),
                        });
                        run_start = k;
                        i = k;
                        matched = true;
                    }
                }
            }
            if !matched {
                // Not a fraction — advance past the digit run.
                while i < chars.len() && chars[i].is_ascii_digit() { i += 1; }
            }
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 3d: journal citation volume/issue/pages
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_citation_volume_pages(text: &str) -> String {
    flatten(&expand_citation_volume_pages_spanned(spanned_from_input(text)))
}

/// True if a 4-digit year (1500-2099) ends at `end` (exclusive) and isn't
/// glued to a preceding alphanumeric.
fn year_ends_at(chars: &[char], end: usize) -> bool {
    if end < 4 || !chars[end - 4..end].iter().all(|c| c.is_ascii_digit()) { return false; }
    if end > 4 && chars[end - 5].is_alphanumeric() { return false; }
    let year: u32 = chars[end - 4..end].iter().collect::<String>().parse().unwrap_or(0);
    (1500..=2099).contains(&year)
}

/// Steps `end` back over a run of at least one whitespace char; returns the
/// new end, or None if `chars[end - 1]` isn't whitespace.
fn skip_space_back(chars: &[char], end: usize) -> Option<usize> {
    let mut e = end;
    while e > 0 && chars[e - 1].is_whitespace() { e -= 1; }
    (e < end).then_some(e)
}

/// Steps `end` back over a run of `lo..=hi` chars matching `pred`; returns
/// the new end, or None if the run is the wrong length.
fn skip_run_back(chars: &[char], end: usize, lo: usize, hi: usize, pred: impl Fn(char) -> bool) -> Option<usize> {
    let mut e = end;
    while e > 0 && pred(chars[e - 1]) { e -= 1; }
    (lo..=hi).contains(&(end - e)).then_some(e)
}

/// True if what precedes `end` is a citation date: a 4-digit year,
/// optionally followed by a month word and then a 1-2 digit day
/// (`2007`, `2007 Jan`, `2007 Jan 5`).
fn citation_date_ends_at(chars: &[char], end: usize) -> bool {
    if year_ends_at(chars, end) { return true; }
    let month_then_year = |e: usize| {
        skip_run_back(chars, e, 3, 9, |c| c.is_alphabetic())
            .and_then(|m| skip_space_back(chars, m))
            .is_some_and(|y| year_ends_at(chars, y))
    };
    if month_then_year(end) { return true; }
    skip_run_back(chars, end, 1, 2, |c| c.is_ascii_digit())
        .and_then(|d| skip_space_back(chars, d))
        .is_some_and(month_then_year)
}

/// Rewrites the `;volume(issue):` part of a journal citation of the form
/// `<year>[ <month>[ <day>]];<volume>[(<issue>)]:<pages>` —
/// `"2007;68:343-349"` -> `"2007, volume sixty eight, pages 343-349"`,
/// `"2007;68(3):343"` -> `"2007, volume sixty eight, issue three, page
/// 343"`. Anchored on a preceding year so a `;`-separated list of scores or
/// ratios (`"3:1; 4:2"`) isn't touched. The pages themselves are left raw
/// for Pass 4/8; the pass only looks ahead to pick "page" vs "pages". A
/// citation shape this doesn't recognize (a `Suppl` label, an `e1234` page)
/// is left unchanged.
fn expand_citation_volume_pages_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_citation_volume_pages_chunk(&chunk)).collect()
}

fn expand_citation_volume_pages_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;

    let spell_cardinal = |digits: &str| match digits.parse::<u64>() {
        Ok(n) => number_to_words(n),
        Err(_) => spell_digit_run(digits),
    };

    for semi in 0..chars.len() {
        if chars[semi] != ';' { continue; }
        let date_end = chars[..semi].iter().rposition(|c| !c.is_whitespace()).map_or(0, |p| p + 1);
        if !citation_date_ends_at(&chars, date_end) { continue; }

        let mut j = semi + 1;
        while j < chars.len() && chars[j].is_whitespace() { j += 1; }
        let vol_start = j;
        while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
        if j == vol_start { continue; }
        let volume: String = chars[vol_start..j].iter().collect();

        let mut issue = None;
        if chars.get(j) == Some(&'(') {
            let issue_start = j + 1;
            let mut k = issue_start;
            while k < chars.len() && chars[k].is_ascii_digit() { k += 1; }
            if k == issue_start || chars.get(k) != Some(&')') { continue; }
            issue = Some(chars[issue_start..k].iter().collect::<String>());
            j = k + 1;
        }
        if chars.get(j) != Some(&':') { continue; }
        let colon = j;
        let Some((pages_end, _)) = scan_number(&chars, colon + 1) else { continue };

        let is_range = chars.get(pages_end).is_some_and(|c| matches!(c, '-' | '\u{2010}' | '\u{2011}' | '\u{2013}'))
            && chars.get(pages_end + 1).is_some_and(|c| c.is_ascii_digit());
        let mut text = format!(", volume {}", spell_cardinal(&volume));
        if let Some(issue) = issue {
            text.push_str(&format!(", issue {}", spell_cardinal(&issue)));
        }
        text.push_str(if is_range { ", pages " } else { ", page " });

        let mut flush_end = semi;
        while flush_end > run_start && chars[flush_end - 1].is_whitespace() { flush_end -= 1; }
        if flush_end > run_start {
            out.push(Spanned {
                text: chars[run_start..flush_end].iter().collect(),
                src: (base + run_start)..(base + flush_end),
            });
        }
        out.push(Spanned { text, src: (base + flush_end)..(base + colon + 1) });
        run_start = colon + 1;
    }
    if chars.len() > run_start {
        out.push(Spanned {
            text: chars[run_start..].iter().collect(),
            src: (base + run_start)..(base + chars.len()),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Pass 4c: ratio expansion
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_ratios(text: &str) -> String {
    flatten(&expand_ratios_spanned(spanned_from_input(text)))
}

/// True for the `H:MM` clock-time shape: a 1-2 digit hour up to 24 and
/// exactly two minute digits up to 59, no comma grouping or decimals on
/// either side. A ratio that happens to fit (`3:15`) is indistinguishable
/// from a time and reads as one.
fn looks_like_clock_time(hour: &str, minutes: &str) -> bool {
    hour.len() <= 2
        && minutes.len() == 2
        && hour.chars().all(|c| c.is_ascii_digit())
        && minutes.chars().all(|c| c.is_ascii_digit())
        && hour.parse::<u32>().is_ok_and(|h| h <= 24)
        && minutes.parse::<u32>().is_ok_and(|m| m <= 59)
}

/// Spells one side of a ratio as a magnitude: `number_to_words` for the
/// integer part (so `16` is "sixteen" and `1990` is "one thousand nine
/// hundred ninety", not the year-style digit-group reading), then ` point `
/// and the decimal digits if there is a decimal part.
fn spell_ratio_side(s: &str) -> String {
    let (int_part, dec_part) = match s.split_once('.') {
        Some((i, d)) => (i, Some(d)),
        None => (s, None),
    };
    let digits: String = int_part.chars().filter(|c| *c != ',').collect();
    let int_words = match digits.parse::<u64>() {
        Ok(n) => number_to_words(n),
        Err(_) => spell_digit_run(&digits),
    };
    match dec_part {
        Some(d) => format!("{int_words} point {}", spell_digit_run(d)),
        None => int_words,
    }
}

/// Span-aware ratio expansion.
fn expand_ratios_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_ratios_chunk(&chunk)).collect()
}

fn expand_ratios_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let Some((first_end, first)) = scan_number(&chars, start) else { break };
        i = first_end;

        // A letter before the number means an alphanumeric identifier; a
        // ':' preceded by a digit means we're mid-chain ("1:30:00").
        let preceded_by_letter = start > 0 && chars[start - 1].is_alphabetic();
        let mid_chain = start >= 2 && chars[start - 1] == ':' && chars[start - 2].is_ascii_digit();
        // A `;` before the number (optionally spaced) is the journal
        // citation shape `2007;68:343-349` — volume:pages, not a ratio.
        let after_semicolon = chars[..start]
            .iter()
            .rev()
            .find(|c| !c.is_whitespace())
            .is_some_and(|&c| c == ';');
        if preceded_by_letter || mid_chain || after_semicolon || chars.get(first_end) != Some(&':') {
            continue;
        }
        let Some((second_end, second)) = scan_number(&chars, first_end + 1) else { continue };
        i = second_end;

        // Three or more segments is a duration/identifier chain, not a
        // ratio; a trailing letter is part of a larger token.
        let chain_continues = chars.get(second_end) == Some(&':')
            && chars.get(second_end + 1).is_some_and(|c| c.is_ascii_digit());
        let followed_by_letter = chars.get(second_end).is_some_and(|c| c.is_alphabetic());
        if chain_continues || followed_by_letter { continue; }
        if looks_like_id(&first) || looks_like_id(&second) { continue; }
        if looks_like_clock_time(&first, &second) { continue; }

        flush(&mut out, &chars, base, run_start, start);
        out.push(Spanned {
            text: format!("{} to {}", spell_ratio_side(&first), spell_ratio_side(&second)),
            src: (base + start)..(base + second_end),
        });
        run_start = second_end;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

/// Spells a bare digit run as words, for any length — `spell_bare_digit_group`
/// only covers 1-4 digit runs; a longer run (or one it otherwise can't
/// handle) falls back to spelling each digit individually, the same
/// fallback `split_alphanumeric` uses. Once a caller expands a match into
/// non-raw text, no later pass gets another chance at digits left bare
/// inside it — a silent `unwrap_or(original)` here would leave them
/// unspoken and invisible to forced alignment, not just unspelled.
fn spell_digit_run(digits: &str) -> String {
    spell_bare_digit_group(digits)
        .unwrap_or_else(|| digits.chars().map(digit_word).collect::<Vec<_>>().join(" "))
}

/// Span-aware replacement of hyphens with spaces — the trivial case: every
/// output char maps 1:1 to the same-position source char, so chunk source
/// ranges pass through unchanged. Superseded in the real pipeline by the
/// inline hyphen→space handling in `tokenize_chunk`; kept as the original
/// proof-of-concept test fixture for the 1:1-transform shape.
#[cfg(test)]
fn hyphens_to_spaces_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().map(|chunk| {
        let text: String = chunk.text.chars().map(|c| if c == '-' { ' ' } else { c }).collect();
        Spanned { text, src: chunk.src }
    }).collect()
}

/// Span-aware bracket stripping.
///
/// A bracket glued directly onto a word on both sides — no space either
/// way, e.g. `"Men(Philadelphia"` or `"1782 (Chester"` once the digits
/// expand and the `(` ends up glued to `"Chester"` — must become a space,
/// not a plain deletion, or the two words fuse into one unreadable token
/// (`"MenPhiladelphia"`).
///
/// For `(`/`)` specifically, a bracket that isn't glued on both sides but
/// sits between real prose and a word — a space-delimited parenthetical
/// aside like `"Arch (I had reached..."` — is replaced with a comma
/// instead of plain deletion, mirroring how Pass 5 treats an unglued
/// em-dash as an aside boundary rather than dropping it silently. Found via
/// a real sentence, `"Admiralty Arch (I had reached that monument), or..."`
/// — plain deletion left `"Arch I had"` with no separator at all, which
/// TTS read as `"Arch the First"` (an unintended Roman-numeral/ordinal
/// reading of the bare `"I"`). Only fires when the *outer* neighbor (the
/// word before `(`, or after `)`) is alphanumeric with no pause punctuation
/// already there (a comma/period/etc. already provides the pause, so no
/// second one is added — e.g. `"monument), or"` stays a plain deletion),
/// and the *inner* neighbor (the char actually glued to the bracket, going
/// into the parenthetical) is also alphanumeric — that inner-alphanumeric
/// requirement is what keeps empty parens (`"word () other"`) from
/// producing a stray double comma. `<`, `>`, `[`, `]` are unaffected — they
/// stay on the old plain-deletion/space behavior.
///
/// This can't be a simple per-chunk scan like `replace_em_dash_chunk`:
/// `strip_brackets_spanned` runs after Pass 10 tokenization, which splits
/// every non-alphanumeric character (including each bracket) into its own
/// 1-char chunk — so a bracket's chunk never contains its neighboring word,
/// only cross-chunk lookup can see it. Same reasoning as
/// `strip_short_quotes_spanned` above: flatten once, decide per bracket
/// position using the flattened neighbors, then rebuild chunks from that
/// decision — deleting a position, swapping it for a single space, or (for
/// an aside-boundary paren) a comma, whichever the flattened context calls
/// for.
fn strip_brackets_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    let flat: Vec<char> = flatten(&chunks).chars().collect();
    let is_bracket = |c: char| matches!(c, '(' | ')' | '<' | '>' | '[' | ']');
    let is_pause_punct = |c: char| matches!(c, ',' | '.' | '!' | '?' | ';' | ':');

    let mut to_remove: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut to_space: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut to_comma: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (idx, &c) in flat.iter().enumerate() {
        if !is_bracket(c) { continue; }
        let glued_both_sides = idx > 0 && flat[idx - 1].is_alphanumeric()
            && idx + 1 < flat.len() && flat[idx + 1].is_alphanumeric();
        if glued_both_sides {
            to_space.insert(idx);
            continue;
        }

        if c == '(' || c == ')' {
            // Nearest non-whitespace char on the *outer* side (away from
            // the parenthetical content): before `(`, after `)`.
            let outer = if c == '(' {
                let mut j = idx;
                while j > 0 && flat[j - 1].is_whitespace() { j -= 1; }
                if j > 0 { Some((j - 1, flat[j - 1])) } else { None }
            } else {
                let mut j = idx + 1;
                while j < flat.len() && flat[j].is_whitespace() { j += 1; }
                if j < flat.len() { Some((j, flat[j])) } else { None }
            };
            // The char actually glued to the bracket on the *inner* side
            // (going into the parenthetical content).
            let inner_alnum = if c == '(' {
                flat.get(idx + 1).is_some_and(|ch| ch.is_alphanumeric())
            } else {
                idx.checked_sub(1).and_then(|i| flat.get(i)).is_some_and(|ch| ch.is_alphanumeric())
            };

            if let Some((outer_idx, outer_ch)) = outer {
                if outer_ch.is_alphanumeric() && inner_alnum {
                    // Real prose on the outer side, no existing pause —
                    // treat this bracket as an aside boundary: drop the
                    // gap whitespace and replace the bracket with a comma
                    // (glued to the outer word; the inner side, having no
                    // whitespace of its own by construction, gets its
                    // separating space from the comma's own trailing space).
                    if c == '(' {
                        for k in (outer_idx + 1)..idx { to_remove.insert(k); }
                    } else {
                        for k in (idx + 1)..outer_idx { to_remove.insert(k); }
                    }
                    to_comma.insert(idx);
                    continue;
                } else if is_pause_punct(outer_ch) {
                    to_remove.insert(idx);
                    continue;
                }
            }
        }
        to_remove.insert(idx);
    }
    if to_remove.is_empty() && to_space.is_empty() && to_comma.is_empty() { return chunks; }

    let mut out = Vec::new();
    let mut flat_pos = 0;
    for chunk in chunks {
        let chunk_chars: Vec<char> = chunk.text.chars().collect();
        let mut piece_start = 0;
        for local_i in 0..chunk_chars.len() {
            let global_i = flat_pos + local_i;
            if to_remove.contains(&global_i) {
                if local_i > piece_start {
                    out.push(subchunk(&chunk, &chunk_chars, piece_start, local_i));
                }
                piece_start = local_i + 1;
            } else if to_space.contains(&global_i) {
                if local_i > piece_start {
                    out.push(subchunk(&chunk, &chunk_chars, piece_start, local_i));
                }
                let src = if is_raw(&chunk) {
                    (chunk.src.start + local_i)..(chunk.src.start + local_i + 1)
                } else {
                    chunk.src.clone()
                };
                out.push(Spanned { text: " ".to_owned(), src });
                piece_start = local_i + 1;
            } else if to_comma.contains(&global_i) {
                if local_i > piece_start {
                    out.push(subchunk(&chunk, &chunk_chars, piece_start, local_i));
                }
                let src = if is_raw(&chunk) {
                    (chunk.src.start + local_i)..(chunk.src.start + local_i + 1)
                } else {
                    chunk.src.clone()
                };
                out.push(Spanned { text: ", ".to_owned(), src });
                piece_start = local_i + 1;
            }
        }
        if piece_start < chunk_chars.len() {
            out.push(subchunk(&chunk, &chunk_chars, piece_start, chunk_chars.len()));
        }
        flat_pos += chunk_chars.len();
    }
    out
}

// ---------------------------------------------------------------------------
// Pass 11b: drop bare `|` tokens
// ---------------------------------------------------------------------------

/// Drops every whitespace-delimited token made only of `|` characters,
/// along with the whitespace that separated it from its neighbor so no
/// double space is left behind (`"a | b"` -> `"a b"`, `"| Role 1: |"` ->
/// `"Role 1:"`). A `|` inside a word or expression (`"∫|H(f)|^2"`) is
/// left alone — only a pipe with whitespace (or the text edge) on both
/// sides is a separator.
///
/// Plain text carries these when a markdown table didn't parse (the whole
/// block falls through as one raw-pipe paragraph) or from a byline
/// (`"Written by X | Published Y"`). They are not spoken, but each one is a
/// whitespace-delimited token, so it would count as a word everywhere
/// segments are sized. Runs after Pass 10, which turns a table's `|---|`
/// separator row into bare `|` tokens separated by spaces.
fn strip_bare_pipes_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    let flat: Vec<char> = flatten(&chunks).chars().collect();
    let mut to_remove: std::collections::HashSet<usize> = std::collections::HashSet::new();

    let mut i = 0;
    while i < flat.len() {
        if flat[i] != '|' {
            i += 1;
            continue;
        }
        let run_start = i;
        while i < flat.len() && flat[i] == '|' { i += 1; }
        let run_end = i;

        let bounded_before = run_start == 0 || flat[run_start - 1].is_whitespace();
        let bounded_after = run_end == flat.len() || flat[run_end].is_whitespace();
        if !(bounded_before && bounded_after) { continue; }

        to_remove.extend(run_start..run_end);
        // Take the separating whitespace with it: the run's trailing
        // whitespace when something follows, otherwise (the run ends the
        // text) the whitespace before it.
        let mut after = run_end;
        while after < flat.len() && flat[after].is_whitespace() { to_remove.insert(after); after += 1; }
        i = after;
        if after == flat.len() {
            let mut before = run_start;
            while before > 0 && flat[before - 1].is_whitespace() { before -= 1; to_remove.insert(before); }
        }
    }
    if to_remove.is_empty() { return chunks; }

    let mut out = Vec::new();
    let mut flat_pos = 0;
    for chunk in chunks {
        let chunk_chars: Vec<char> = chunk.text.chars().collect();
        let mut piece_start = 0;
        for local_i in 0..chunk_chars.len() {
            if to_remove.contains(&(flat_pos + local_i)) {
                if local_i > piece_start {
                    out.push(subchunk(&chunk, &chunk_chars, piece_start, local_i));
                }
                piece_start = local_i + 1;
            }
        }
        if piece_start < chunk_chars.len() {
            out.push(subchunk(&chunk, &chunk_chars, piece_start, chunk_chars.len()));
        }
        flat_pos += chunk_chars.len();
    }
    out
}

// ---------------------------------------------------------------------------
// Pass 12: capitalize sentence start
// ---------------------------------------------------------------------------

#[cfg(test)]
fn capitalize_sentence_start(text: &str) -> String {
    flatten(&capitalize_sentence_start_spanned(spanned_from_input(text)))
}

/// Capitalizes the first character of the normalized output if it's a
/// lowercase letter, unconditionally — not gated on whether that chunk was
/// produced by an earlier pass, since only pronunciation matters here (see
/// the call site in `normalize_with_spans`). Only the first non-empty chunk
/// can possibly contain char 0, so every other chunk is left untouched.
fn capitalize_sentence_start_spanned(mut chunks: Vec<Spanned>) -> Vec<Spanned> {
    if let Some(first) = chunks.iter_mut().find(|c| !c.text.is_empty()) {
        let mut chars = first.text.chars();
        let head = chars.next().expect("checked non-empty above");
        if head.is_lowercase() {
            first.text = head.to_uppercase().collect::<String>() + chars.as_str();
        }
    }
    chunks
}

fn replace_ellipsis_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| replace_ellipsis_chunk(&chunk)).collect()
}

/// Length of an ellipsis marker starting at `chars[i]`, or `None` if there
/// isn't one there. Three shapes, since real documents use whichever
/// typographic convention interchangeably and all read the same way:
/// - The single `…` codepoint.
/// - A contiguous run of 3+ periods (`...`, `....`) — not just exactly
///   three, so a longer run doesn't leave a leftover bare `.` behind.
/// - The same run spaced out, one space between each period (`". . ."`,
///   `". . . ."`) — found in a real document as `"right now. . . . A
///   concern"`, which the contiguous-only check completely missed,
///   leaving every one of those periods to reach the TTS backend as
///   literal, unhandled punctuation.
fn ellipsis_match_len(chars: &[char], i: usize) -> Option<usize> {
    if chars[i] == '\u{2026}' { return Some(1); }
    if chars[i] != '.' { return None; }

    let mut contiguous = 1;
    while chars.get(i + contiguous) == Some(&'.') { contiguous += 1; }
    if contiguous >= 3 { return Some(contiguous); }

    let mut len = 1;
    let mut dots = 1;
    while chars.get(i + len) == Some(&' ') && chars.get(i + len + 1) == Some(&'.') {
        len += 2;
        dots += 1;
    }
    (dots >= 3).then_some(len)
}

/// Handles every ellipsis marker — see `ellipsis_match_len` for the shapes
/// recognized — mirroring `replace_em_dash_chunk`'s shape.
///
/// Two earlier approaches to this pass didn't hold up: replacing `...` with
/// a newline (to give VibeVoice a pause boundary instead of it treating the
/// sentence as unfinished and looping) caused a different artifact, a
/// "taunt" mispronunciation; stripping the marker to nothing entirely
/// avoided both, but left no separator at all when the marker sat directly
/// between two words (`"paused...then"` collapsed to `"pausedthen"`) — a
/// real regression discovered later, since deleting the marker isn't the
/// same as deleting the *pause* it represented.
///
/// A leading space is always consumed as part of the match, the same as
/// `replace_em_dash_chunk` — `"paused ... then"` must become
/// `"paused then"`, not `"paused  then"` (a stray double space is its own
/// TTS artifact). What follows the marker decides the replacement:
/// - A closing quote immediately after is consumed and kept as-is
///   (`foo..."` -> `foo"`) — a quote isn't a word needing separation.
/// - Any run of whitespace right after the marker is consumed too, so this
///   pass decides for itself whether exactly one space belongs there rather
///   than trusting however much was in the source.
/// - If what's left is alphanumeric (more prose follows) and this isn't the
///   very start of the sentence, exactly one space is inserted —
///   `"paused...then"` and `"paused... then"` both become `"paused then"`.
/// - Otherwise (punctuation follows, end of text, or the marker opened the
///   sentence) nothing is inserted; the marker just attaches directly
///   (`"every ..., and"` -> `"every, and"`).
fn replace_ellipsis_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        let Some(elen) = ellipsis_match_len(&chars, i) else { i += 1; continue; };

        let has_space_before = i > 0 && chars[i - 1].is_whitespace();
        let match_start = if has_space_before { i - 1 } else { i };

        let mut replacement = String::new();
        let mut after = i + elen;
        if chars.get(after) == Some(&'"') {
            replacement.push('"');
            after += 1;
        }
        while chars.get(after).is_some_and(|c| c.is_whitespace()) { after += 1; }

        let at_sentence_start = base + match_start == 0;
        if !at_sentence_start && chars.get(after).is_some_and(|c| c.is_alphanumeric()) {
            replacement.push(' ');
        }

        flush(&mut out, &chars, base, run_start, match_start);
        if !replacement.is_empty() {
            out.push(Spanned { text: replacement, src: (base + match_start)..(base + after) });
        }
        run_start = after;
        i = after;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

/// Span-aware version of `replace_identifier_dots`. Each matched `.` (a
/// single source char) expands to `" dot "` (identifier notation like
/// `4b.l`, `Ref.dt`) or `" point "` when digits flank both sides (`26.44`,
/// `3.14`) — same flush/expand shape as `expand_numeric_ranges_chunk`, just
/// with a 1-char source range per expansion instead of a multi-char one.
fn replace_identifier_dots_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| replace_identifier_dots_chunk(&chunk)).collect()
}

fn replace_identifier_dots_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    for i in 0..chars.len() {
        if chars[i] == '.'
            && (i == 0 || chars[i - 1] != '.')
            && i + 1 < chars.len() && chars[i + 1].is_alphanumeric()
        {
            // This pass runs before Pass 6d/7 spell the surrounding digits
            // out, so both neighbors are still raw digit chars here —
            // decide point-vs-dot straight off them, no lookback needed.
            let flanked_by_digits =
                i > 0 && chars[i - 1].is_ascii_digit() && chars[i + 1].is_ascii_digit();
            let word = if flanked_by_digits { " point " } else { " dot " };
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned { text: word.to_owned(), src: (base + i)..(base + i + 1) });
            run_start = i + 1;
        } else if chars[i] == '@' && i + 1 < chars.len() && chars[i + 1].is_alphanumeric() {
            // Email ("support@example.us") or handle ("@exampleus"): read "@"
            // as "at", with a leading space only when glued to a preceding
            // letter/digit.
            let glued_before = i > 0 && chars[i - 1].is_alphanumeric();
            let word = if glued_before { " at " } else { "at " };
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned { text: word.to_owned(), src: (base + i)..(base + i + 1) });
            run_start = i + 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

/// Replace em-dashes and double-hyphens with a comma so TTS gets a clean
/// pause cue instead of extra whitespace that causes vocalisation artifacts.
#[cfg(test)]
fn replace_em_dash_and_double_hyphen(text: &str) -> String {
    flatten(&replace_em_dash_and_double_hyphen_spanned(spanned_from_input(text)))
}

/// Span-aware version. First drops a leading em-dash (see
/// `drop_leading_em_dash`), then scans remaining em-dashes one at a time
/// (see `replace_em_dash_chunk`) for the drop-vs-comma decision, then
/// handles " -- " (4 chars) -> ", " (2 chars) via `replace_literal_spanned`
/// — that pattern's mandatory surrounding spaces mean it's always
/// already well-formed, unlike the bare em-dash char.
fn replace_em_dash_and_double_hyphen_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    let chunks = drop_leading_em_dash(chunks);
    let chunks = chunks.into_iter().flat_map(|chunk| replace_em_dash_chunk(&chunk)).collect();
    replace_literal_spanned(chunks, " -- ", ", ")
}

/// Handles every remaining (non-leading) em-dash. A leading space is always
/// consumed as part of the match — `"lines — things"` must become
/// `"Lines, things"`, not `"lines , things"` (space-before-comma is its own
/// TTS artifact) — but what replaces the dash depends on what precedes it:
///
/// - A comma is only inserted when the (space-adjusted) preceding char is
///   alphanumeric. Any non-alphanumeric preceding char — sentence-terminal
///   punctuation (`.`, `!`, `?`, `…`), a comma, a colon, or anything else
///   punctuation-shaped — already reads as a pause or a boundary on its
///   own, so the dash is **dropped entirely** instead, no comma inserted.
///   Stacking a second pause-marker right after one that's already there
///   is redundant and just as artifact-prone as the glued-comma case:
///   `"wedding.… —Jo Reed"` must become `"wedding. Jo Reed"`, not
///   `"wedding.,Jo Reed"`; `"card,—refused"` must become `"card,
///   refused"`, not `"card,, refused"`; `"problem:—"` must become
///   `"problem: "`, not `"problem:,"`. A space is inserted only if
///   neither side of the dash already has one, to avoid gluing the words
///   on either side.
/// - Otherwise, the dash becomes a comma — with a trailing space added if
///   the dash had none (the `—Author` attribution convention has no space
///   after the dash), so the comma doesn't glue onto the next word.
fn replace_em_dash_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    for i in 0..chars.len() {
        if chars[i] != '\u{2014}' { continue; }

        let has_space_before = i > 0 && chars[i - 1].is_whitespace();
        let has_space_after = i + 1 < chars.len() && chars[i + 1].is_whitespace();

        let prev_idx = if has_space_before { i.checked_sub(2) } else { i.checked_sub(1) };
        let comma_warranted = prev_idx
            .and_then(|idx| chars.get(idx))
            .is_some_and(|c| c.is_alphanumeric());

        let match_start = if has_space_before { i - 1 } else { i };
        let replacement = if !comma_warranted {
            // The leading space, if any, is always consumed as part of the
            // match above — only the trailing space (left untouched) can
            // still serve as a separator, so that's the only one that
            // matters here.
            if has_space_after { String::new() } else { " ".to_owned() }
        } else if has_space_after {
            ",".to_owned()
        } else {
            ", ".to_owned()
        };

        flush(&mut out, &chars, base, run_start, match_start);
        if !replacement.is_empty() {
            out.push(Spanned { text: replacement, src: (base + match_start)..(base + i + 1) });
        }
        run_start = i + 1;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

/// A `—` at the very start of the sentence (e.g. `—William Penn, 1694,
/// in Britain Yearly Meeting...`) is a block-quote attribution marker, not
/// a mid-sentence pause — there's nothing preceding it to pause after.
/// Converting it to a comma the way the mid-sentence case does would glue
/// the comma directly onto the first word (this convention has no space
/// after the dash either), producing a TTS artifact instead of a clean
/// reading. Drop it instead of replacing it.
fn drop_leading_em_dash(mut chunks: Vec<Spanned>) -> Vec<Spanned> {
    let should_drop = match chunks.first() {
        Some(first) => is_raw(first) && first.src.start == 0 && first.text.starts_with('\u{2014}'),
        None => false,
    };
    if should_drop {
        let first = &mut chunks[0];
        first.text.remove(0);
        first.src.start += 1;
        if first.text.is_empty() {
            chunks.remove(0);
        }
    }
    chunks
}

// ---------------------------------------------------------------------------
// Pass 8: bare number expansion (comma-grouped and digit-group)
// ---------------------------------------------------------------------------

/// Expand comma-grouped numbers like "2,000" or "100,000" into words
/// ("two thousand", "one hundred thousand"). Requires each group after the
/// first comma to have exactly 3 digits, and the whole number not to be
/// adjacent to other digits/letters (so "Ref-1,000" style codes are left
/// alone).
#[cfg(test)]
fn expand_comma_numbers(text: &str) -> String {
    flatten(&expand_comma_numbers_spanned(spanned_from_input(text)))
}

/// Span-aware version of `expand_comma_numbers` — same flush/expand shape
/// as `expand_numeric_ranges_chunk`: an unexpanded digit run (no valid
/// comma-grouping found) falls through and stays queued in the pending
/// unprocessed range; an expanded run gets the source range of the whole
/// digit-comma sequence it replaced.
fn expand_comma_numbers_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_comma_numbers_chunk(&chunk)).collect()
}

fn expand_comma_numbers_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if chars[i].is_ascii_digit() && (i == 0 || !chars[i - 1].is_ascii_digit()) {
            let start = i;
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }

            // A `-` directly against this digit run's start is a negative
            // sign, not a hyphen, when nothing alphanumeric sits right
            // before it — that excludes a subtraction/range like "5-3"
            // (Pass 4 already consumed a genuine year range before this
            // pass runs, so a digit-`-`-digit shape still raw here isn't
            // one) and a hyphenated code like "F30602-76". Left
            // unhandled, the digits below still get spelled ("-1" ->
            // "one"), but the sign itself becomes invisible: nothing later
            // in the pipeline treats a bare `-` as meaningful, so Pass 10's
            // generic hyphen-to-space rule silently blanks it — "-1, -2"
            // reads as "one, two", indistinguishable from "1, 2".
            let sign_start = (start > 0 && chars[start - 1] == '-'
                && (start < 2 || !chars[start - 2].is_alphanumeric()))
                .then(|| start - 1);
            let word_start = sign_start.unwrap_or(start);
            let sign_prefix = if sign_start.is_some() { "negative " } else { "" };

            let mut end = j;
            while end < chars.len() && chars[end] == ','
                && end + 3 < chars.len()
                && chars[end + 1..end + 4].iter().all(|c| c.is_ascii_digit())
                && (end + 4 == chars.len() || !chars[end + 4].is_ascii_digit())
            {
                end += 4;
            }
            if end > j {
                let digits: String = chars[start..end].iter().filter(|c| **c != ',').collect();
                if let Ok(n) = digits.parse::<u64>() {
                    flush(&mut out, &chars, base, run_start, word_start);
                    out.push(Spanned {
                        text: format!("{sign_prefix}{}", number_to_words(n)),
                        src: (base + word_start)..(base + end),
                    });
                    run_start = end;
                    i = end;
                    continue;
                }
            }
            // Not comma-grouped. A bare 1-4 digit number gets spelled out
            // ("5" -> "five", "42" -> "forty two", "560" -> "five sixty",
            // "1976" -> "nineteen seventy six") rather than being read as
            // raw digits — which TTS tends to garble, and which forced
            // alignment can't time-align at all (its vocabulary has no
            // digit characters, so bare numbers get silently dropped).
            // Also excluded here: a digit run touching a letter on either
            // side ("4b", "14B", "v2", "512K") — not skipped entirely, just
            // deferred: split_alphanumeric (in process_token) splits these
            // later in the pipeline and spells out each digit run itself,
            // so the number still gets spelled, just after the letter is
            // separated off rather than here. (Leading-zero exclusion is
            // handled inside `spell_bare_digit_group` itself.)
            let touches_letter = (start > 0 && chars[start - 1].is_alphabetic())
                || (j < chars.len() && chars[j].is_alphabetic());
            if !touches_letter {
                let digits: String = chars[start..j].iter().collect();
                if let Some(words) = spell_bare_digit_group(&digits) {
                    flush(&mut out, &chars, base, run_start, word_start);
                    out.push(Spanned {
                        text: format!("{sign_prefix}{words}"),
                        src: (base + word_start)..(base + j),
                    });
                    run_start = j;
                } else if digits.len() >= 5 {
                    // 5+ digit bare number — spell_bare_digit_group only
                    // covers 1-4 digits. Left as raw digits, this was
                    // silently invisible to forced alignment too (no
                    // letters in its vocabulary). Falls back to spelling
                    // each digit individually, the same fallback already
                    // used elsewhere in this file (Pass 3's <Ref-N> tags,
                    // Pass 3b's footnote markers, Pass 4's numeric ranges)
                    // whenever a number is too long for the digit-group
                    // convention. A 2-4 digit leading-zero run ("012",
                    // "0609") still falls through untouched here — that
                    // stays Pass 10's job (`tokenize_chunk`'s leading-zero
                    // rule), unchanged by this fallback.
                    flush(&mut out, &chars, base, run_start, word_start);
                    let spelled = digits.chars().map(digit_word).collect::<Vec<_>>().join(" ");
                    out.push(Spanned {
                        text: format!("{sign_prefix}{spelled}"),
                        src: (base + word_start)..(base + j),
                    });
                    run_start = j;
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

/// Spell out a bare 3-digit number "digit-group" style: the hundreds digit
/// read alone, then the remaining two digits read as a unit. E.g. "560" ->
/// "five sixty", "501" -> "five oh one" (leading zero in the last two
/// digits, address/phone convention), "500" -> "five hundred" (round).
fn three_digit_group_words(digits: &str) -> String {
    let mut chars = digits.chars();
    let hundreds = digit_word(chars.next().expect("3-digit number"));
    let rest = chars.as_str(); // remaining 2 digits, e.g. "60", "01", "00"

    if rest == "00" {
        format!("{hundreds} hundred")
    } else if rest.starts_with('0') {
        let ones = digit_word(rest.chars().nth(1).expect("2-digit remainder"));
        format!("{hundreds} oh {ones}")
    } else {
        let rest_n: u64 = rest.parse().expect("2 ascii digits");
        format!("{hundreds} {}", number_to_words(rest_n))
    }
}

/// Spell out a bare 4-digit number "digit-group" style, as years and
/// addresses are normally read: two 2-digit groups, each read as its own
/// number. E.g. "1976" -> "nineteen seventy six", "2010" -> "twenty ten".
///
/// Special cases for round numbers (second group "00"):
/// - Round thousand ("2000", "3000" — first group itself ends in 0) reads
///   as a single cardinal: "two thousand", "three thousand". Splitting
///   these into groups would give the wrong "twenty hundred".
/// - Round hundred-within-thousand ("1900", "2300") reads as
///   "<first group> hundred": "nineteen hundred", "twenty three hundred".
///
/// A leading zero in the second group ("1905") reads as "oh <digit>":
/// "nineteen oh five".
fn four_digit_group_words(digits: &str) -> String {
    let first = &digits[0..2];
    let second = &digits[2..4];
    let group_words = |s: &str| number_to_words(s.parse::<u64>().expect("2 ascii digits"));

    if second == "00" {
        if first.ends_with('0') {
            let n: u64 = digits.parse().expect("4 ascii digits");
            number_to_words(n)
        } else {
            format!("{} hundred", group_words(first))
        }
    } else if second.starts_with('0') {
        let ones = digit_word(second.chars().nth(1).expect("2-digit group"));
        format!("{} oh {ones}", group_words(first))
    } else {
        format!("{} {}", group_words(first), group_words(second))
    }
}

/// Spell out a bare digit-only string using the same convention as the
/// generic bare-number rule (Pass 8): 1-2 digits via `number_to_words`,
/// 3 via `three_digit_group_words`, 4 via `four_digit_group_words`.
/// Returns `None` for forms this convention doesn't cover — 5+ digits, or
/// a leading-zero run longer than 1 char (those are IDs, spelled
/// digit-by-digit elsewhere — a 2-4 digit leading-zero run in
/// `process_token`, a 5+ digit run whether leading-zero or not directly in
/// Pass 8 itself) — so callers can fall back to their own digit-by-digit
/// spelling.
///
/// Shared by Pass 8 itself and by `expand_numeric_ranges_chunk` (Pass 4),
/// which needs to spell out both halves of a range like "1973-76" itself
/// rather than leaving bare digits for Pass 8 to find — once Pass 4 expands
/// "1973-76" into "1973 to 76", that text is no longer raw, so Pass 8's
/// scan correctly skips it (per the chunk-granularity design), and the
/// embedded digits would otherwise stay bare forever: not just unspoken
/// awkwardly, but invisible to forced alignment (whose vocabulary has no
/// digit characters), entirely dropping that word from alignment results.
pub fn spell_bare_digit_group(digits: &str) -> Option<String> {
    let len = digits.chars().count();
    if !matches!(len, 1 | 2 | 3 | 4) { return None; }
    if len > 1 && digits.starts_with('0') { return None; }
    Some(match len {
        1 | 2 => number_to_words(digits.parse().expect("1-2 ascii digits")),
        3 => three_digit_group_words(digits),
        _ => four_digit_group_words(digits),
    })
}

// ---------------------------------------------------------------------------
// Pass 0b: strip space before comma and other medial/closing punctuation
// ---------------------------------------------------------------------------

/// Punctuation marks (other than `.`, handled separately by Pass 5c) that a
/// stray preceding space should be glued to. Grouped with the comma case in
/// Pass 0b since none of them interact with Pass 5b's ellipsis-chunking
/// concern — that's specific to `.`.
const SPACE_BEFORE_PUNCT_EARLY: &[char] = &[',', '!', '?', ';', ')', ']'];

#[cfg(test)]
fn strip_space_before_comma(text: &str) -> String {
    flatten(&strip_space_before_comma_spanned(spanned_from_input(text)))
}

/// Span-aware version. Any run of whitespace immediately before a comma (or
/// `!`/`?`/`;`/`)`/`]` — see `SPACE_BEFORE_PUNCT_EARLY`) is dropped, leaving
/// the mark glued to whatever precedes it — a stray space there
/// (`"Press , 1948"`, `"in Mexico City ( above ),"`) has no meaning worth
/// preserving, and left in, it makes the TTS backend vocalize a strange
/// nonsense syllable.
fn strip_space_before_comma_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter()
        .flat_map(|chunk| strip_space_before_punctuation_chunk(&chunk, SPACE_BEFORE_PUNCT_EARLY))
        .collect()
}

// ---------------------------------------------------------------------------
// Pass 0b2: strip space after an opening bracket
// ---------------------------------------------------------------------------

#[cfg(test)]
fn strip_space_after_opening_bracket(text: &str) -> String {
    flatten(&strip_space_after_opening_bracket_spanned(spanned_from_input(text)))
}

/// Span-aware version. Mirror image of Pass 0b for `(`/`[`: any run of
/// whitespace immediately after one of these is dropped, leaving it glued to
/// whatever follows — `"( above )"` combines with Pass 0b's `)` handling to
/// become `"(above)"`.
fn strip_space_after_opening_bracket_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| strip_space_after_punctuation_chunk(&chunk, &['(', '['])).collect()
}

// ---------------------------------------------------------------------------
// Pass 5c: strip space before period
// ---------------------------------------------------------------------------

#[cfg(test)]
fn strip_space_before_period(text: &str) -> String {
    flatten(&strip_space_before_period_spanned(spanned_from_input(text)))
}

/// Span-aware version. Same artifact and fix as Pass 0b's comma case
/// (`"the end ."` -> `"the end."`), but for a period specifically — kept as
/// a separate, later pass rather than folded into Pass 0b, since Pass 0b
/// runs before Pass 5b (ellipsis). Stripping a space before *every* period
/// that early would split a spaced-out ellipsis (`". . . ."`, see Pass 5b)
/// into several single-character chunks at each removed space — each too
/// short on its own for Pass 5b's contiguous-run scan to recognize, so the
/// dots would reach the TTS backend as literal, unmerged punctuation
/// instead of a pause. Running this pass after Pass 5b sidesteps the
/// conflict entirely: any spaced ellipsis has already been resolved by
/// then, so every period this pass still sees is a genuine, isolated one.
fn strip_space_before_period_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| strip_space_before_punctuation_chunk(&chunk, &['.'])).collect()
}

/// Shared by Pass 0b (comma + `!?;)]`) and Pass 5c (period): drops any run of
/// whitespace immediately before one of `punct`, leaving it glued to
/// whatever precedes it.
fn strip_space_before_punctuation_chunk(chunk: &Spanned, punct: &[char]) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if chars[i].is_whitespace() {
            let space_start = i;
            while i < chars.len() && chars[i].is_whitespace() { i += 1; }
            if chars.get(i).is_some_and(|c| punct.contains(c)) {
                flush(&mut out, &chars, base, run_start, space_start);
                run_start = i;
                continue;
            }
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

/// Mirror image of `strip_space_before_punctuation_chunk`: drops any run of
/// whitespace immediately after one of `punct`, leaving it glued to whatever
/// follows.
fn strip_space_after_punctuation_chunk(chunk: &Spanned, punct: &[char]) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if punct.contains(&chars[i]) {
            let punct_end = i + 1;
            let mut j = punct_end;
            while j < chars.len() && chars[j].is_whitespace() { j += 1; }
            if j > punct_end {
                flush(&mut out, &chars, base, run_start, punct_end);
                run_start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 5d: strip trailing quotes after terminal punctuation
// ---------------------------------------------------------------------------

#[cfg(test)]
fn strip_trailing_quotes_after_terminal_punctuation(text: &str) -> String {
    flatten(&strip_trailing_quotes_after_terminal_punctuation_spanned(spanned_from_input(text)))
}

/// Span-aware version. Any run of `'`/`"` immediately after a terminal
/// punctuation mark (`.`, `!`, `?`) is dropped entirely, leaving the mark
/// itself in place — quotes carry no pronunciation, and Pass 2 already
/// discards them wholesale rather than trying to preserve balance/nesting,
/// so a leftover closing-quote cluster right after a sentence ends is
/// treated the same way rather than kept paired.
fn strip_trailing_quotes_after_terminal_punctuation_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| strip_trailing_quotes_chunk(&chunk)).collect()
}

fn strip_trailing_quotes_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if matches!(chars[i], '.' | '!' | '?') {
            let punct_end = i + 1;
            let mut j = punct_end;
            while j < chars.len() && (chars[j] == '\'' || chars[j] == '"') { j += 1; }
            if j > punct_end {
                flush(&mut out, &chars, base, run_start, punct_end);
                run_start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 0c: normalize curly quotes
// ---------------------------------------------------------------------------

#[cfg(test)]
fn normalize_curly_quotes(text: &str) -> String {
    flatten(&normalize_curly_quotes_spanned(spanned_from_input(text)))
}

/// Maps curly/smart quotes to their ASCII equivalents — one character for
/// one character, so this never changes a chunk's length and can map
/// blindly over every chunk (raw or not) without needing to flush/subdivide
/// like most other passes here.
fn normalize_curly_quotes_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().map(|chunk| {
        let text: String = chunk.text.chars().map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            other => other,
        }).collect();
        Spanned { text, src: chunk.src }
    }).collect()
}

// ---------------------------------------------------------------------------
// Pass 0d: drop bare colon
// ---------------------------------------------------------------------------

#[cfg(test)]
fn normalize_bare_colon(text: &str) -> String {
    flatten(&normalize_bare_colon_spanned(spanned_from_input(text)))
}

/// Rewrites a colon that's acting as sentence punctuation rather than a
/// digit separator, in three cases:
/// - Preceded by a letter, at the end of the text (only whitespace after it)
///   (`"the output buffer y:"` -> `"the output buffer y."`) — replaced with a
///   period. The literal colon produces the same garbled vocalization as
///   below; a period rather than a comma so the sentence ends with a
///   terminal cadence instead of a hanging, unfinished one.
/// - Preceded by a letter, followed by whitespace (`"Note: something"` ->
///   `"Note, something"`) — replaced with a comma. A bare colon reaching the
///   TTS backend as literal punctuation produces a short garbled
///   vocalization instead of a real pause; dropping it outright (an earlier
///   version of this pass) was tried, but turned out awkward more often than
///   useful across a real document's colons — a comma in its place reads
///   more naturally.
/// - Preceded by a letter, followed directly by a digit with no whitespace
///   (`"arXiv:2301.02111"` -> `"arXiv 2301.02111"`) — replaced with a single
///   space. Found via a real `arXiv:<id>` citation: the `arXiv` override
///   (Pass 1) only rewrites the word token, so the untouched colon glued the
///   replacement word straight onto the following spelled-out digits with
///   no separation at all.
///
/// Deliberately narrow: a colon between digits (`"3:00"`, a time; `"2:1"`,
/// a ratio) has a digit, not a letter, on at least one side, so it's left
/// alone either way.
fn normalize_bare_colon_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    let last = chunks.len().saturating_sub(1);
    chunks.into_iter().enumerate()
        .flat_map(|(n, chunk)| normalize_bare_colon_chunk(&chunk, n == last))
        .collect()
}

fn normalize_bare_colon_chunk(chunk: &Spanned, is_last: bool) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    for i in 0..chars.len() {
        if chars[i] != ':' || i == 0 || !chars[i - 1].is_alphabetic() {
            continue;
        }
        if is_last && chars[i + 1..].iter().all(|n| n.is_whitespace()) {
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned { text: ".".to_owned(), src: (base + i)..(base + i + 1) });
            run_start = i + 1;
        } else if chars.get(i + 1).is_some_and(|n| n.is_whitespace()) {
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned { text: ",".to_owned(), src: (base + i)..(base + i + 1) });
            run_start = i + 1;
        } else if chars.get(i + 1).is_some_and(|n| n.is_ascii_digit()) {
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned { text: " ".to_owned(), src: (base + i)..(base + i + 1) });
            run_start = i + 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 0e: drop on-the-hour minutes before AM/PM
// ---------------------------------------------------------------------------

#[cfg(test)]
fn drop_on_the_hour_minutes(text: &str) -> String {
    flatten(&drop_on_the_hour_minutes_spanned(spanned_from_input(text)))
}

/// Drops `:00` from a clock time immediately before `AM`/`PM`
/// (case-insensitive, optional single space) — `"9:00 AM"` -> `"9 AM"`,
/// `"11:00PM"` -> `"11PM"`. Scoped tightly to the AM/PM-adjacent case so a
/// duration like `"5:00 remaining"` (not a clock time) is left alone.
fn drop_on_the_hour_minutes_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| drop_on_the_hour_minutes_chunk(&chunk)).collect()
}

fn drop_on_the_hour_minutes_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if chars[i].is_ascii_digit() && (i == 0 || !chars[i - 1].is_ascii_digit()) {
            let hour_start = i;
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
            let hour_len = j - hour_start;

            let is_colon_00 = (1..=2).contains(&hour_len)
                && chars.get(j) == Some(&':')
                && chars.get(j + 1) == Some(&'0') && chars.get(j + 2) == Some(&'0')
                && !chars.get(j + 3).is_some_and(|c| c.is_ascii_digit());

            if is_colon_00 {
                let mut k = j + 3;
                let ws_start = k;
                if chars.get(k) == Some(&' ') { k += 1; }
                let after: String = chars[k..].iter().take(2).collect();
                if after.eq_ignore_ascii_case("am") || after.eq_ignore_ascii_case("pm") {
                    flush(&mut out, &chars, base, run_start, j);
                    // Always exactly one space before AM/PM, whether or not
                    // the source had one — "11:00PM" (no space) must not
                    // come out "elevenP M" once the hour is spelled out.
                    out.push(Spanned { text: " ".to_owned(), src: (base + ws_start)..(base + k) });
                    run_start = k;
                    i = k;
                    continue;
                }
            }
        }
        i += 1;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 0f: degree/minute/second coordinates
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_degree_minute_second(text: &str) -> String {
    flatten(&expand_degree_minute_second_spanned(spanned_from_input(text)))
}

/// Expands a degrees-minutes(-seconds) coordinate group — `"42° 15′"` ->
/// `"forty two degrees fifteen minutes"`, `"31° 15′ 20″"` -> `"...twenty
/// seconds"`. Anchored specifically on the arcminute/arcsecond prime
/// characters (`′` U+2032, `″` U+2033), never the ASCII apostrophe/quote —
/// those are also used for feet/inches (`5′ 6″` tall), and there's no way
/// to tell the two usages apart from the character alone. Requiring a
/// minutes group directly after a degree group is what disambiguates: a
/// bare `"5′"` with no preceding `"<digits>°"` is left completely alone.
///
/// Minutes are required to fire at all — a bare `"72°"` with no minutes
/// group following (e.g. a temperature) is left for the generic `°` ->
/// "degrees" override (in the built-in override table) to handle on its own. Seconds
/// are optional on top of a matched degrees+minutes group.
fn expand_degree_minute_second_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_degree_minute_second_chunk(&chunk)).collect()
}

fn expand_degree_minute_second_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    // Scans "<digits><ws>*<marker>" starting at j. Returns (position right
    // after the marker, the digit text) on a match.
    fn scan_group(chars: &[char], j: usize, marker: char) -> Option<(usize, String)> {
        let start = j;
        let mut k = j;
        while k < chars.len() && chars[k].is_ascii_digit() { k += 1; }
        if k == start { return None; }
        let digits: String = chars[start..k].iter().collect();
        let mut m = k;
        while m < chars.len() && chars[m] == ' ' { m += 1; }
        if chars.get(m) == Some(&marker) { Some((m + 1, digits)) } else { None }
    }

    while i < chars.len() {
        if chars[i].is_ascii_digit() && (i == 0 || !chars[i - 1].is_ascii_digit()) {
            let group_start = i;
            let matched = 'matched: {
                let Some((after_deg, deg_digits)) = scan_group(&chars, i, '\u{b0}') else { break 'matched None };
                let mut j = after_deg;
                while j < chars.len() && chars[j] == ' ' { j += 1; }
                let Some((after_min, min_digits)) = scan_group(&chars, j, '\u{2032}') else { break 'matched None };

                let mut expanded = format!(
                    "{} degrees {} minutes",
                    spell_digit_run(&deg_digits),
                    spell_digit_run(&min_digits),
                );
                let mut end = after_min;
                let mut k = after_min;
                while k < chars.len() && chars[k] == ' ' { k += 1; }
                if let Some((after_sec, sec_digits)) = scan_group(&chars, k, '\u{2033}') {
                    expanded.push_str(&format!(" {} seconds", spell_digit_run(&sec_digits)));
                    end = after_sec;
                }
                Some((end, expanded))
            };
            if let Some((end, expanded)) = matched {
                flush(&mut out, &chars, base, run_start, group_start);
                out.push(Spanned { text: expanded, src: (base + group_start)..(base + end) });
                run_start = end;
                i = end;
                continue;
            }
            while i < chars.len() && chars[i].is_ascii_digit() { i += 1; }
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 0: URL links
// ---------------------------------------------------------------------------

#[cfg(test)]
fn replace_urls(text: &str) -> String {
    flatten(&replace_urls_spanned(spanned_from_input(text)))
}

/// Span-aware version. Matches a literal `http://` or `https://` scheme,
/// then extends to the next whitespace (URLs don't contain spaces). One
/// trailing sentence-punctuation character (`.`, `,`, `;`, `:`, `!`, `?`,
/// `)`) is trimmed off the match if present, since real documents glue
/// their own sentence punctuation directly onto the end of a URL with no
/// space (`https://example.com/page.`) — trimming it keeps the sentence's
/// own punctuation intact rather than swallowing it into "link".
fn replace_urls_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| replace_urls_chunk(&chunk)).collect()
}

fn replace_urls_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        let lookahead: String = chars[i..].iter().take(8).collect();
        if lookahead.starts_with("http://") || lookahead.starts_with("https://") {
            let mut end = i;
            while end < chars.len() && !chars[end].is_whitespace() { end += 1; }
            if end > i + 7 && matches!(chars[end - 1], '.' | ',' | ';' | ':' | '!' | '?' | ')') {
                end -= 1;
            }
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned { text: "link".to_owned(), src: (base + i)..(base + end) });
            run_start = end;
            i = end;
            continue;
        }
        i += 1;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 1: handle punctuated overrides before token processing, so they can
// match across punctuation (e.g. `e.g.`).
// ---------------------------------------------------------------------------

#[cfg(test)]
fn apply_punctuated_overrides(text: &str, overrides: &HashMap<String, String>) -> String {
    flatten(&apply_punctuated_overrides_spanned(spanned_from_input(text), &punctuated_keys(overrides)))
}

/// A punctuated override entry as Pass 1 matches it.
struct PunctuatedKey {
    /// The key after Passes 0–0f, so it looks like the text Pass 1 sees:
    /// `(this display)!!"` is matched as `(this display)!!`, since Pass 0c2
    /// drops a closing quote after `!`.
    matched: String,
    replacement: String,
    /// A key with an uppercase letter (e.g. "No.") is matched case-
    /// sensitively — the whole point of writing it that way instead of
    /// "no." is to scope the override to the abbreviation usage and leave
    /// the ordinary lowercase word ("...is no.") untouched. A key with no
    /// uppercase letters is matched case-insensitively.
    case_sensitive: bool,
}

/// The punctuated entries of `overrides` (keys that aren't all letters,
/// digits and apostrophes; word-only keys are Pass 10's), longest first, so
/// a key that contains another ("C++" and "+") is applied before it.
fn punctuated_keys(overrides: &HashMap<String, String>) -> Vec<PunctuatedKey> {
    let mut keys: Vec<PunctuatedKey> = overrides.iter()
        .filter(|(k, _)| !k.chars().all(|c| c.is_alphanumeric() || c == '\''))
        .map(|(k, v)| PunctuatedKey {
            matched: flatten(&early_passes_spanned(spanned_from_input(k))),
            replacement: v.clone(),
            case_sensitive: k.chars().any(|c| c.is_uppercase()),
        })
        .collect();
    keys.sort_by(|a, b| b.matched.chars().count().cmp(&a.matched.chars().count()).then_with(|| a.matched.cmp(&b.matched)));
    keys
}

/// Span-aware version of `apply_punctuated_overrides`. Each override key is
/// applied as its own full pass over the chunk list (mirroring the unspanned
/// version's one-pass-per-key loop), so spans stay correctly anchored even
/// when multiple override keys apply to overlapping-looking text in sequence.
fn apply_punctuated_overrides_spanned(chunks: Vec<Spanned>, punctuated: &[PunctuatedKey]) -> Vec<Spanned> {
    let mut chunks = chunks;
    for key in punctuated {
        chunks = over_raw_runs(chunks, |run| apply_one_override_run(run, &key.matched, &key.replacement, key.case_sensitive, None));
    }
    for (from, singular, plural) in NUMBER_UNITS {
        chunks = over_raw_runs(chunks, |run| apply_one_override_run(run, from, singular, false, Some(plural)));
    }
    chunks
}

/// Calls `f` on each run of consecutive raw chunks, passing non-raw chunks
/// through. An earlier pass that removes a character (Pass 0c2's dropped
/// quote, Pass 0b's dropped space) leaves the text on either side in
/// separate raw chunks, so a key has to be matched across them.
fn over_raw_runs(chunks: Vec<Spanned>, f: impl Fn(&[Spanned]) -> Vec<Spanned>) -> Vec<Spanned> {
    let mut out = Vec::with_capacity(chunks.len());
    let mut run: Vec<Spanned> = Vec::new();
    for chunk in chunks {
        if is_raw(&chunk) {
            run.push(chunk);
        } else {
            if !run.is_empty() { out.extend(f(&run)); run.clear(); }
            out.push(chunk);
        }
    }
    if !run.is_empty() { out.extend(f(&run)); }
    out
}

/// Unit abbreviations that are only expanded directly after a number, as
/// (abbreviation, singular word, plural word). Word-only override keys
/// can't be scoped that way (they match every occurrence, so `ms` would
/// also rewrite "Ms"), so these ride the same number-lookback as "in." in
/// Pass 1 instead.
///
/// Some of these also have a word override in the built-in table ("dB" ->
/// "decibels"), which still speaks the abbreviation when no number comes
/// before it ("measured in dB").
const NUMBER_UNITS: &[(&str, &str, &str)] = &[
    ("ms", "millisecond", "milliseconds"),
    ("µs", "microsecond", "microseconds"),
    // Greek small mu (U+03BC), as opposed to the micro sign (U+00B5) above.
    ("μs", "microsecond", "microseconds"),
    ("ns", "nanosecond", "nanoseconds"),
    ("kb", "kilobyte", "kilobytes"),
    ("mb", "megabyte", "megabytes"),
    ("gb", "gigabyte", "gigabytes"),
    ("db", "decibel", "decibels"),
    ("cm", "centimeter", "centimeters"),
    ("cm2", "square centimeter", "square centimeters"),
    ("ft", "foot", "feet"),
    ("lb", "pound", "pounds"),
    ("lbs", "pound", "pounds"),
];

/// `unit_plural` is the plural word for a `NUMBER_UNITS` key, `None` for
/// any other key.
/// `run` is consecutive raw chunks (see `over_raw_runs`); a match may span
/// several of them. Unmatched text keeps its original chunk boundaries.
fn apply_one_override_run(run: &[Spanned], from: &str, to: &str, case_sensitive: bool, unit_plural: Option<&str>) -> Vec<Spanned> {
    let number_unit = unit_plural.is_some();
    // Each char of the run, with its source position and the chunk it
    // came from.
    let mut chars: Vec<char> = Vec::new();
    let mut pos: Vec<usize> = Vec::new();
    let mut owner: Vec<usize> = Vec::new();
    for (n, chunk) in run.iter().enumerate() {
        for (k, c) in chunk.text.chars().enumerate() {
            chars.push(c);
            pos.push(chunk.src.start + k);
            owner.push(n);
        }
    }
    let from_chars: Vec<char> = from.chars().collect();
    let from_len = from_chars.len();
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    // Pushes chars[start..end], split where the original chunks were.
    let flush = |out: &mut Vec<Spanned>, start: usize, end: usize| {
        let mut s = start;
        while s < end {
            let mut e = s + 1;
            while e < end && owner[e] == owner[s] { e += 1; }
            out.push(Spanned { text: chars[s..e].iter().collect(), src: pos[s]..(pos[e - 1] + 1) });
            s = e;
        }
    };

    // Require a word boundary before the match when `from` starts with an
    // alphanumeric char, so e.g. "p." doesn't fire inside "Scholarship."
    // (the "p" + sentence-final "." just happens to spell the key).
    let needs_boundary = from_chars.first().is_some_and(|c| c.is_alphanumeric());

    // "p."/"pp." are only ever a page-citation abbreviation when followed by
    // a number ("p. 234", "pp. 99-108") — real prose also uses bare "p" as a
    // single-letter variable name (e.g. "observes sound pressure p."), where
    // the word-boundary check above still lets the key match since nothing
    // alphanumeric precedes it. Requiring a following digit (skipping any
    // whitespace) disambiguates without touching every other override.
    // "No." gets the same check: the case-sensitive key already skips
    // lowercase "no." ending a sentence, but a sentence that is just "No."
    // ("No. These two…") is capitalized too, and only "No. 7" is the
    // citation abbreviation.
    let requires_trailing_digit = from.eq_ignore_ascii_case("p.") || from.eq_ignore_ascii_case("pp.") || from == "No.";

    // "in." is only ever the inch abbreviation right after a number
    // ("3 in.", "12in."); otherwise it's the word "in" ending a sentence
    // ("keeps you signed in."). When it does match, the value is
    // pluralized for any number other than a bare "1", and the period is
    // kept if it also ends the sentence (end of chunk, or whitespace then
    // an uppercase letter) — the key consumes that period, which would
    // otherwise drop the sentence's terminal punctuation.
    let is_inch = from.eq_ignore_ascii_case("in.");

    // `number_unit` keys (see `NUMBER_UNITS`) share the "in." number
    // lookback, but have no period of their own, so they also need a
    // trailing word boundary ("5 msg" is not milliseconds) and the number
    // must end in a digit ("see 3. ms" is not 3 milliseconds).
    let number_gated = is_inch || number_unit;

    while i < chars.len() {
        let mut matches = from_len > 0 && i + from_len <= chars.len()
            && chars[i..i + from_len].iter().zip(from_chars.iter())
                .all(|(a, b)| if case_sensitive { a == b } else { a.to_lowercase().eq(b.to_lowercase()) })
            && (!needs_boundary || i == 0 || !chars[i - 1].is_alphanumeric()
                || (number_gated && chars[i - 1].is_ascii_digit()))
            && (!number_unit || !chars.get(i + from_len).is_some_and(|c| c.is_alphanumeric()));
        if matches && requires_trailing_digit {
            let mut j = i + from_len;
            while chars.get(j).is_some_and(|c| c.is_whitespace()) { j += 1; }
            matches = chars.get(j).is_some_and(|c| c.is_ascii_digit());
        }
        let mut inch_number = String::new();
        if matches && number_gated {
            let mut j = i;
            while j > 0 && chars[j - 1].is_whitespace() { j -= 1; }
            let num_end = j;
            while j > 0 && (chars[j - 1].is_ascii_digit() || matches!(chars[j - 1], '.' | ',' | '/')) { j -= 1; }
            inch_number = chars[j..num_end].iter().collect();
            matches = if number_unit {
                inch_number.chars().last().is_some_and(|c| c.is_ascii_digit())
            } else {
                inch_number.chars().any(|c| c.is_ascii_digit())
            };
        }
        if matches {
            flush(&mut out, run_start, i);
            // A replacement glued directly onto a letter/digit on either
            // side (e.g. "72°F" -> "degrees" glued between "72" and "F")
            // would otherwise fuse into one unreadable run
            // ("72degreesF") — insert a separating space on whichever
            // side(s) are actually glued. Doesn't fire for the common
            // case (space or other punctuation already on that side).
            let glued_before = i > 0 && chars[i - 1].is_alphanumeric();
            let glued_after = chars.get(i + from_len).is_some_and(|c| c.is_alphanumeric());
            let mut replacement = to.to_owned();
            if let Some(plural) = unit_plural.filter(|_| inch_number != "1") {
                replacement = plural.to_owned();
            }
            if is_inch {
                if inch_number != "1" { replacement = format!("{to}es"); }
                let mut j = i + from_len;
                while chars.get(j).is_some_and(|c| c.is_whitespace()) { j += 1; }
                if chars.get(j).is_none_or(|c| c.is_uppercase()) { replacement.push('.'); }
            }
            if glued_before { replacement.insert(0, ' '); }
            if glued_after { replacement.push(' '); }
            out.push(Spanned { text: replacement, src: pos[i]..(pos[i + from_len - 1] + 1) });
            run_start = i + from_len;
            i += from_len;
        } else {
            i += 1;
        }
    }
    flush(&mut out, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 1b: AM/PM after a number
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_am_pm_after_number(text: &str) -> String {
    flatten(&expand_am_pm_after_number_spanned(spanned_from_input(text)))
}

/// Spells `am`/`pm` (any case, a whole word) as `"A M"`/`"P M"` when the
/// nearest non-whitespace char before it is a digit: `"9 am"`, `"5pm"`,
/// `"10:30 PM"`. Anywhere else it's left alone, so the word "am" ("I am")
/// and "PM" as a title ("PM Modi") keep their own pronunciation. A word
/// override can't do this, since it matches every occurrence.
///
/// The lookback crosses chunk boundaries: Pass 0e turns `"11:00PM"` into
/// `"11"`, an inserted `" "`, and `"PM"` as separate chunks.
fn expand_am_pm_after_number_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    let mut out = Vec::new();
    let mut prev_char: Option<char> = None;
    let mut prev_non_ws: Option<char> = None;
    for chunk in chunks {
        let start = out.len();
        if is_raw(&chunk) {
            out.extend(expand_am_pm_chunk(&chunk, prev_char, prev_non_ws));
        } else {
            out.push(chunk);
        }
        for c in &out[start..] {
            if let Some(last) = c.text.chars().last() { prev_char = Some(last); }
            if let Some(last) = c.text.chars().rev().find(|c| !c.is_whitespace()) { prev_non_ws = Some(last); }
        }
    }
    out
}

fn expand_am_pm_chunk(chunk: &Spanned, prev_char: Option<char>, prev_non_ws: Option<char>) -> Vec<Spanned> {
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i + 1 < chars.len() {
        let letter = chars[i].to_ascii_uppercase();
        let is_am_pm = matches!(letter, 'A' | 'P') && chars[i + 1].eq_ignore_ascii_case(&'m');
        let before = if i > 0 { Some(chars[i - 1]) } else { prev_char };
        // A digit right before is fine ("5pm"); a letter means it's part of
        // a longer word ("dream", "spam").
        let starts_word = before.is_none_or(|c| !c.is_alphanumeric() || c.is_ascii_digit());
        let ends_word = chars.get(i + 2).is_none_or(|c| !c.is_alphanumeric());
        let after_number = is_am_pm && starts_word && ends_word && {
            let mut j = i;
            while j > 0 && chars[j - 1].is_whitespace() { j -= 1; }
            let c = if j > 0 { Some(chars[j - 1]) } else { prev_non_ws };
            c.is_some_and(|c| c.is_ascii_digit())
        };
        if after_number {
            flush(&mut out, &chars, base, run_start, i);
            let mut replacement = format!("{letter} M");
            if before.is_some_and(|c| c.is_ascii_digit()) { replacement.insert(0, ' '); }
            out.push(Spanned { text: replacement, src: (base + i)..(base + i + 2) });
            run_start = i + 2;
            i += 2;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

/// Generic span-aware exact-match literal substring replace (case-sensitive,
/// unlike `apply_one_override_chunk`). Used by any pass that needs to
/// replace or remove (`to = ""`) a fixed substring while staying scannable
/// by later passes — i.e. instead of blindly mapping a chunk's whole text
/// (which silently breaks the text/src length invariant the moment the
/// replacement isn't 1:1), this flushes/subdivides like the other
/// expansion passes so untouched text around each match stays "raw" and
/// available for further matching.
fn replace_literal_spanned(chunks: Vec<Spanned>, from: &str, to: &str) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| replace_literal_chunk(&chunk, from, to)).collect()
}

fn replace_literal_chunk(chunk: &Spanned, from: &str, to: &str) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let from_chars: Vec<char> = from.chars().collect();
    let from_len = from_chars.len();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if from_len > 0 && i + from_len <= chars.len() && chars[i..i + from_len] == from_chars[..] {
            flush(&mut out, &chars, base, run_start, i);
            if !to.is_empty() {
                out.push(Spanned { text: to.to_owned(), src: (base + i)..(base + i + from_len) });
            }
            run_start = i + from_len;
            i += from_len;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 10: token processing
// ---------------------------------------------------------------------------

fn process_token(token: &str, overrides: &HashMap<String, String>) -> String {
    if let Some(replacement) = overrides.get(&token.to_lowercase()) {
        return replacement.clone();
    }

    let has_digit = token.chars().any(|c| c.is_ascii_digit());
    let has_alpha = token.chars().any(|c| c.is_alphabetic());
    if has_digit && has_alpha {
        return split_alphanumeric(token);
    }

    if has_alpha && !has_digit {
        return process_alpha_token(token);
    }

    // Leading-zero numbers ("0609", "069") are IDs, not magnitudes — spell
    // each digit as a word so TTS reads them clearly.
    if has_digit && token.len() > 1 && token.starts_with('0') {
        return token.chars().map(digit_word).collect::<Vec<_>>().join(" ");
    }

    token.to_owned()
}

/// Splits an alphanumeric token into digit/letter runs, spelling out each
/// digit run as words (e.g. "512K" -> "five twelve K", "4c2" -> "four c
/// two") instead of leaving raw digits — raw digits are hard for TTS to
/// pronounce reliably and forced alignment has no digit characters in its
/// vocabulary, so they'd otherwise go untimed. `spell_bare_digit_group`
/// handles the common 1-4 digit cases the same way bare numbers elsewhere
/// in the pipeline are spelled; runs it doesn't cover (longer runs, or a
/// leading zero with len > 1, e.g. an ID like "007") fall back to spelling
/// each digit individually.
fn split_alphanumeric(token: &str) -> String {
    let mut groups: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut prev_kind: Option<bool> = None;

    for ch in token.chars() {
        let is_digit = ch.is_ascii_digit();
        if let Some(prev) = prev_kind {
            if prev != is_digit {
                groups.push(std::mem::take(&mut current));
            }
        }
        current.push(ch);
        prev_kind = Some(is_digit);
    }
    if !current.is_empty() { groups.push(current); }

    groups.into_iter()
        .map(|g| if g.chars().all(|c| c.is_ascii_digit()) { spell_digit_run(&g) } else { g })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Spell out a number in words, e.g. 1999 -> "one thousand nine hundred
/// ninety nine". Drops num2words' hyphens ("ninety-nine") and "and"
/// ("hundred and ninety-nine") to match this normalizer's plain
/// space-separated style.
pub fn number_to_words(n: u64) -> String {
    let words = num2words::Num2Words::new(n)
        .to_words()
        .unwrap_or_else(|_| n.to_string());
    words
        .replace('-', " ")
        .split(' ')
        .filter(|w| *w != "and")
        .collect::<Vec<_>>()
        .join(" ")
}

/// Common short function words (articles, conjunctions, prepositions, a
/// few pronouns/verbs) that must never be spelled out letter-by-letter even
/// when all-caps — they're never real acronyms, just caught up in an
/// ALL-CAPS TITLE HEADING's styling (`"THE"`, `"TO"`, `"FOR"`, `"IN"`).
/// Only checked inside the `all_caps` branch below, so a normal mixed-case
/// `"The"` never reaches this code path at all — ordinary capitalization
/// is untouched.
const SHORT_COMMON_WORDS: &[&str] = &[
    "a", "an", "and", "as", "at", "be", "but", "by", "for", "if", "in", "is",
    "it", "nor", "not", "of", "off", "on", "or", "out", "per", "so", "the",
    "to", "up", "via", "we", "yet",
];

fn process_alpha_token(token: &str) -> String {
    let (stem, suffix) = if let Some(s) = token.strip_suffix("'s")
        .or_else(|| token.strip_suffix("'S"))
    {
        (s, &token[s.len()..])
    } else {
        (token, "")
    };

    let alpha_count = stem.chars().filter(|c| c.is_alphabetic()).count();
    let all_caps = stem.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase());

    // Roman numeral check: skip single chars (I, V, X etc. are too ambiguous).
    // Only all-caps stems are converted — lowercase Roman numerals ("xxx",
    // "yy") are indistinguishable from lowercase placeholder strings (e.g.
    // "(DDD,xxx,bb)" in authorship.txt), which are far more common in our
    // target documents. Disambiguating "xiv" (= 14) from a placeholder would
    // need per-document overrides; not implemented.
    // A 2-letter stem is only plausibly a numeral when it uses just I/V/X
    // (II..XX): pairs with L/C/D/M ("ML", "MD", "DC") are far more often
    // author initials or abbreviations than numerals of 40 or more.
    let plausible_roman = alpha_count != 2
        || stem.chars().filter(|c| c.is_alphabetic()).all(|c| matches!(c, 'I' | 'V' | 'X'));
    if alpha_count >= 2 && all_caps && plausible_roman {
        if let Some(n) = roman::from(stem) {
            let words = number_to_words(n as u64);
            return if suffix.is_empty() { words } else { format!("{words}{}", suffix.to_lowercase()) };
        }
    }

    if all_caps {
        if alpha_count <= 3 && !SHORT_COMMON_WORDS.contains(&stem.to_lowercase().as_str()) {
            // Spell out only the alphabetic core, re-attaching whatever
            // leading/trailing non-alphabetic characters (e.g. quotes around
            // a quoted acronym like "'I'") the token carried — the
            // tokenizer keeps those attached to support contractions, but
            // they aren't letters to spell out.
            let chars: Vec<char> = stem.chars().collect();
            let lead_len = chars.iter().take_while(|c| !c.is_alphabetic()).count();
            let trail_len = chars.iter().rev().take_while(|c| !c.is_alphabetic()).count()
                .min(chars.len() - lead_len);
            let lead: String = chars[..lead_len].iter().collect();
            let trail: String = chars[chars.len() - trail_len..].iter().collect();
            let core: String = chars[lead_len..chars.len() - trail_len].iter().collect();
            let spelled = core.chars().map(|c| c.to_string()).collect::<Vec<_>>().join(" ");
            let result = format!("{lead}{spelled}{trail}");
            if suffix.is_empty() { result } else { format!("{}{}", result, suffix.to_lowercase()) }
        } else {
            // Title-case: capitalize first letter so "GENERAL" → "General"
            // rather than "general". Sounds the same to TTS but reads more
            // naturally and avoids issues when preceded by punctuation (e.g. "A. General").
            let mut chars = stem.chars();
            let titled = match chars.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
            };
            format!("{}{}", titled, suffix.to_lowercase())
        }
    } else {
        token.to_owned()
    }
}

// ---------------------------------------------------------------------------
// Pass 10: tokenize on word boundaries, process each token, replace
// remaining hyphens with spaces.
// ---------------------------------------------------------------------------

#[cfg(test)]
fn tokenize(text: &str, overrides: &HashMap<String, String>) -> String {
    flatten(&tokenize_spanned(spanned_from_input(text), overrides))
}

/// Span-aware version of `tokenize`. Each token's whole char range becomes
/// the source span for whatever `process_token` outputs (per the
/// chunk-granularity design — we don't try to map individual output words
/// within a multi-word expansion back to individual input chars). Each
/// separator char (including hyphen→space) maps 1:1, like
/// `hyphens_to_spaces_spanned`.
fn tokenize_spanned(chunks: Vec<Spanned>, overrides: &HashMap<String, String>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| tokenize_chunk(&chunk, overrides)).collect()
}

fn tokenize_chunk(chunk: &Spanned, overrides: &HashMap<String, String>) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut tok_start: Option<usize> = None;

    fn flush_token(out: &mut Vec<Spanned>, chars: &[char], base: usize, tok_start: &mut Option<usize>, end: usize, overrides: &HashMap<String, String>) {
        if let Some(start) = tok_start.take() {
            let token: String = chars[start..end].iter().collect();
            let processed = process_token(&token, overrides);
            out.push(Spanned { text: processed, src: (base + start)..(base + end) });
        }
    }

    for i in 0..chars.len() {
        let ch = chars[i];
        if ch.is_alphanumeric() || ch == '\'' {
            if tok_start.is_none() { tok_start = Some(i); }
        } else {
            flush_token(&mut out, &chars, base, &mut tok_start, i, overrides);
            // A hyphen glued directly onto a following comma (no space
            // between) carries no content of its own to speak — it's an
            // elided-suffix marker ("16-, 32- or 64-bit") or similar, not a
            // word-joining hyphen. Turning it into a space would strand a
            // space directly before the comma, the same TTS artifact Pass
            // 0b exists to avoid — so it's dropped outright instead,
            // leaving the comma glued to whatever precedes it.
            //
            // A hyphen immediately followed by whitespace ("32- or") is
            // dropped the same way rather than turned into a second space
            // — the source's own following space already provides the
            // separation, so converting the hyphen too would double it up
            // ("thirty two  or").
            let next_already_separates = chars.get(i + 1)
                .is_some_and(|&c| c == ',' || c.is_whitespace());
            if ch == '-' && next_already_separates {
                continue;
            }
            let out_ch = if ch == '-' { ' ' } else { ch };
            out.push(Spanned { text: out_ch.to_string(), src: (base + i)..(base + i + 1) });
        }
    }
    flush_token(&mut out, &chars, base, &mut tok_start, chars.len(), overrides);
    out
}

// ---------------------------------------------------------------------------
// Pass 0c2: tighten quote spacing
// ---------------------------------------------------------------------------

#[cfg(test)]
fn tighten_quote_spacing(text: &str) -> String {
    flatten(&tighten_quote_spacing_spanned(spanned_from_input(text)))
}

/// Drops a stray space immediately *after* an opening `"` (`'" Hi"'` ->
/// `'"Hi"'`) and a stray space immediately *before* a closing `"`
/// (`'"hello, " he said'` -> `'"hello," he said'`) — the same OCR/authoring
/// artifact Pass 0b/5c fix for comma/period, just for the quote's own
/// side of the gap. Scoped to `"` only, not `'`: a single quote is also the
/// contraction/possessive apostrophe (`"don't"`, `"Bob's"`), so there's no
/// reliable way to tell a real opening/closing single-quote apart from one
/// of those without much deeper analysis — same reasoning `strip_short_quotes`
/// (Pass 2) already uses to scope itself to `"` only.
///
/// Since ASCII `"` doesn't distinguish opener from closer, this walks the
/// flattened text once (same reason as `strip_short_quotes_spanned` — an
/// earlier pass can split a quote pair across chunks) and treats each `"` as
/// alternating opener/closer by simple occurrence order: 1st, 3rd, 5th, ...
/// are openers, 2nd, 4th, 6th, ... are closers. An unbalanced/malformed
/// quote in the source can throw this off — the same "uneven quote
/// stripping" limitation Pass 2 already has.
///
/// One deliberate exception to that alternating rule: since normalization
/// runs per-sentence, a quoted passage spanning multiple sentences lands its
/// lone closing `"` in a sentence with no matching opener at all
/// (`"...the end. \""`) — the single-quote-in-this-chunk case. A single `"`
/// with nothing but whitespace after it is always treated as a closer
/// (space stripped before it) rather than the default opener assignment,
/// since an opener with nothing following it in the whole chunk isn't a
/// sentence that could ever be produced by a real document.
///
/// A closing `"` found directly after a terminal mark (`.`/`!`/`?`), once
/// the whitespace between them is stripped, is dropped outright rather than
/// left glued — this is exactly what Pass 5d does for a quote that was
/// *already* glued in the source, but Pass 5d can't be relied on to finish
/// the job here: it scans one chunk at a time, and stripping the whitespace
/// above splits the output into separate chunks right at that boundary (the
/// same "flush, then continue" pattern every space-stripping pass in this
/// file uses), so Pass 5d's later per-chunk scan can never see the mark and
/// the quote as adjacent even though the flattened text now reads that way.
///
/// Must run after Pass 0c (curly quotes already normalized to ASCII `"`).
fn tighten_quote_spacing_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    let flat: Vec<char> = flatten(&chunks).chars().collect();
    let quote_positions: Vec<usize> = flat.iter().enumerate()
        .filter(|(_, &c)| c == '"')
        .map(|(i, _)| i)
        .collect();

    let lone_quote_is_closer = quote_positions.len() == 1
        && !flat[quote_positions[0] + 1..].iter().any(|c| !c.is_whitespace());

    let mut to_remove: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (idx, &pos) in quote_positions.iter().enumerate() {
        let is_opener = if quote_positions.len() == 1 { !lone_quote_is_closer } else { idx % 2 == 0 };
        if is_opener {
            let mut j = pos + 1;
            while j < flat.len() && flat[j].is_whitespace() {
                to_remove.insert(j);
                j += 1;
            }
        } else {
            let mut j = pos;
            while j > 0 && flat[j - 1].is_whitespace() && !to_remove.contains(&(j - 1)) {
                to_remove.insert(j - 1);
                j -= 1;
            }
            // Gluing the quote directly onto a preceding terminal mark is
            // exactly what Pass 5d otherwise drops — but Pass 5d scans one
            // chunk at a time, and removing the whitespace above splits the
            // output into separate chunks right at this boundary (the
            // "flush, then continue" pattern every space-stripping pass here
            // uses), so Pass 5d's later per-chunk scan can never see the two
            // pieces as adjacent. Drop the quote here directly instead of
            // relying on that later pass to see across a boundary this pass
            // itself just created.
            // Not when the quotes hold only punctuation (`(".")`, naming the
            // period itself): there the mark is the quoted text, not the
            // end of a quoted sentence.
            let quotes_only_punct = idx > 0
                && !flat[quote_positions[idx - 1] + 1..j].iter().any(|c| c.is_alphanumeric());
            if j > 0 && matches!(flat[j - 1], '.' | '!' | '?') && !quotes_only_punct {
                to_remove.insert(pos);
            }
        }
    }

    if to_remove.is_empty() { return chunks; }

    let mut out = Vec::new();
    let mut flat_pos = 0;
    for chunk in chunks {
        let chunk_chars: Vec<char> = chunk.text.chars().collect();
        let mut piece_start = 0;
        for local_i in 0..chunk_chars.len() {
            if to_remove.contains(&(flat_pos + local_i)) {
                if local_i > piece_start {
                    out.push(subchunk(&chunk, &chunk_chars, piece_start, local_i));
                }
                piece_start = local_i + 1;
            }
        }
        if piece_start < chunk_chars.len() {
            out.push(subchunk(&chunk, &chunk_chars, piece_start, chunk_chars.len()));
        }
        flat_pos += chunk_chars.len();
    }
    out
}

// ---------------------------------------------------------------------------
// Pass 2: strip short inline quotes
// ---------------------------------------------------------------------------

#[cfg(test)]
fn strip_short_quotes(text: &str) -> String {
    flatten(&strip_short_quotes_spanned(spanned_from_input(text)))
}

/// Span-aware version of `strip_short_quotes`.
///
/// Unlike every other `*_spanned` pass, this one can't process chunks
/// independently: `apply_punctuated_overrides_spanned` runs first (by
/// design — see `normalize_with_spans`), and an override match can split
/// what was one chunk into [prefix, expanded-replacement, suffix], landing
/// an opening and closing quote in two *different* chunks with a non-raw
/// expansion between them. A per-chunk scan would never see the matching
/// closing quote. So this scans the flattened text once to find quote
/// pairs to remove, then deletes exactly those two char positions from
/// whichever chunks they fall in — every other chunk (including non-raw
/// ones in between) is passed through completely unmodified, so this never
/// needs to (and doesn't) recompute spans for already-expanded text.
fn strip_short_quotes_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    let flat: Vec<char> = flatten(&chunks).chars().collect();

    let mut to_remove: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut i = 0;
    while i < flat.len() {
        if flat[i] == '"' {
            let content_start = i + 1;
            let mut j = content_start;
            while j < flat.len() && flat[j] != '"' { j += 1; }
            if j < flat.len() {
                let word_count = flat[content_start..j].iter().collect::<String>()
                    .split_whitespace().count();
                if word_count <= 5 {
                    to_remove.insert(i);
                    to_remove.insert(j);
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }

    if to_remove.is_empty() { return chunks; }

    let mut out = Vec::new();
    let mut flat_pos = 0;
    for chunk in chunks {
        let chunk_chars: Vec<char> = chunk.text.chars().collect();
        let mut piece_start = 0;
        for local_i in 0..chunk_chars.len() {
            if to_remove.contains(&(flat_pos + local_i)) {
                if local_i > piece_start {
                    out.push(subchunk(&chunk, &chunk_chars, piece_start, local_i));
                }
                piece_start = local_i + 1;
            }
        }
        if piece_start < chunk_chars.len() {
            out.push(subchunk(&chunk, &chunk_chars, piece_start, chunk_chars.len()));
        }
        flat_pos += chunk_chars.len();
    }
    out
}

/// Build a sub-piece [start, end) of `chunk`'s text. If `chunk` is still
/// raw, the sub-piece gets a precisely narrowed source range; if it's
/// already non-raw (expanded by an earlier pass), precise sub-ranges
/// aren't meaningful, so the whole piece inherits the parent's full source
/// range (per the chunk-granularity design for already-expanded text).
fn subchunk(chunk: &Spanned, chunk_chars: &[char], start: usize, end: usize) -> Spanned {
    let text: String = chunk_chars[start..end].iter().collect();
    let src = if is_raw(chunk) {
        (chunk.src.start + start)..(chunk.src.start + end)
    } else {
        chunk.src.clone()
    };
    Spanned { text, src }
}

// ---------------------------------------------------------------------------
// Pass 6d: spell Item/reference numbers digit-by-digit
// ---------------------------------------------------------------------------

#[cfg(test)]
fn spell_item_numbers(text: &str) -> String {
    flatten(&spell_item_numbers_spanned(spanned_from_input(text)))
}

/// Span-aware version of `spell_item_numbers`. The matched range covers
/// "Item" + whitespace + digits as one source span, since the expansion
/// ("Item one two three four") isn't word-for-word aligned to the original
/// digits — per the chunk-granularity design, callers get the whole
/// matched range, not a per-digit breakdown.
fn spell_item_numbers_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| spell_item_numbers_chunk(&chunk)).collect()
}

fn spell_item_numbers_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if i + 4 <= chars.len()
            && chars[i] == 'I' && chars[i+1] == 't' && chars[i+2] == 'e' && chars[i+3] == 'm'
            && (i == 0 || !chars[i-1].is_alphanumeric())
        {
            let mut j = i + 4;
            while j < chars.len() && chars[j].is_whitespace() { j += 1; }
            let digit_start = j;
            while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
            let digit_count = j - digit_start;
            if digit_count >= 4 && (j == chars.len() || !chars[j].is_alphanumeric()) {
                flush(&mut out, &chars, base, run_start, i);
                let mut expanded = String::from("Item ");
                for (k, &c) in chars[digit_start..j].iter().enumerate() {
                    if k > 0 { expanded.push(' '); }
                    expanded.push_str(digit_word(c));
                }
                out.push(Spanned { text: expanded, src: (base + i)..(base + j) });
                run_start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

fn digit_word(c: char) -> &'static str {
    match c {
        '0' => "zero", '1' => "one",  '2' => "two",   '3' => "three", '4' => "four",
        '5' => "five", '6' => "six",  '7' => "seven",  '8' => "eight", '9' => "nine",
        _   => "",
    }
}

// ---------------------------------------------------------------------------
// Pass 6e: expand a cent-sign amount glued to a digit run — "1¢" -> "one
// cent", "3¢" -> "three cents"
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_cent_sign(text: &str) -> String {
    flatten(&expand_cent_sign_spanned(spanned_from_input(text)))
}

/// Span-aware version. Same flush/expand shape as Pass 6d/7 — the matched
/// range covers the digit run plus the "¢" as one source span.
fn expand_cent_sign_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_cent_sign_chunk(&chunk)).collect()
}

fn expand_cent_sign_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        let digit_start = i;
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
        let has_digits = j > digit_start
            && (digit_start == 0 || !chars[digit_start - 1].is_alphanumeric());

        if has_digits && j < chars.len() && chars[j] == '¢' {
            let digits: String = chars[digit_start..j].iter().collect();
            let spelled = spell_bare_digit_group(&digits).unwrap_or_else(|| {
                chars[digit_start..j].iter().map(|&c| digit_word(c)).collect::<Vec<_>>().join(" ")
            });
            let unit = if digits == "1" { "cent" } else { "cents" };
            flush(&mut out, &chars, base, run_start, digit_start);
            out.push(Spanned {
                text: format!("{spelled} {unit}"),
                src: (base + digit_start)..(base + j + 1),
            });
            run_start = j + 1;
            i = j + 1;
            continue;
        }

        // A bare "¢" with no leading digit run (e.g. "denoted ¢") has no
        // number to agree with — just name the amount generically. Same
        // word-boundary check as the digit-run case: a symbol glued onto
        // an identifier ("v1¢") is left alone rather than guessed at.
        if chars[i] == '¢' && (i == 0 || !chars[i - 1].is_alphanumeric()) {
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned {
                text: "cents".to_string(),
                src: (base + i)..(base + i + 1),
            });
            run_start = i + 1;
            i += 1;
            continue;
        }
        i += 1;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 6f: expand an ordinal number — a digit run immediately followed by
// "st"/"nd"/"rd"/"th" — "15th" -> "fifteenth", "1st" -> "first"
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_ordinals(text: &str) -> String {
    flatten(&expand_ordinals_spanned(spanned_from_input(text)))
}

/// Span-aware version. Runs before Pass 8 for the same "specific before
/// generic" reason as Pass 6d/6e/7: once this pass claims the digit run, the
/// suffix travels with it as one non-raw chunk, so Pass 8's bare-number scan
/// and Pass 10's `split_alphanumeric` (which would otherwise spell the
/// digits and leave the suffix stranded as a raw, unspoken "th") both
/// correctly skip it.
fn expand_ordinals_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_ordinals_chunk(&chunk)).collect()
}

fn expand_ordinals_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        let digit_start = i;
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
        // A letter directly before the digit run makes it part of an
        // identifier ("F15th"), not a number — same word-boundary check as
        // Pass 4/6d.
        let has_digits = j > digit_start
            && (digit_start == 0 || !chars[digit_start - 1].is_alphanumeric());

        let suffix_lower: Option<String> = if has_digits && j + 1 < chars.len() {
            let two: String = chars[j..j + 2].iter().collect::<String>().to_lowercase();
            if matches!(two.as_str(), "st" | "nd" | "rd" | "th") { Some(two) } else { None }
        } else {
            None
        };
        // A letter right after the suffix ("15thing") means this isn't a
        // standalone ordinal — leave the whole thing alone for later passes.
        let suffix_end = j + 2;
        let word_boundary_after = suffix_end >= chars.len() || !chars[suffix_end].is_alphanumeric();

        if let Some(_suffix) = suffix_lower.filter(|_| word_boundary_after) {
            let digits: String = chars[digit_start..j].iter().collect();
            let spelled = ordinal_word_group(&digits);
            flush(&mut out, &chars, base, run_start, digit_start);
            out.push(Spanned {
                text: spelled,
                src: (base + digit_start)..(base + suffix_end),
            });
            run_start = suffix_end;
            i = suffix_end;
            continue;
        }
        i = if has_digits { j.max(i + 1) } else { i + 1 };
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

/// Spells a bare digit run as an ordinal, reusing the same digit-group
/// convention (`spell_digit_run`) every other bare-number pass in this file
/// shares, so an ordinal reads consistently with its cardinal counterpart
/// elsewhere in the same document (e.g. "15" -> "fifteen" here becomes
/// "fifteenth", the same "fifteen" Pass 8 would produce for a bare "15").
/// Only the last word of the spelled-out cardinal needs to change form.
fn ordinal_word_group(digits: &str) -> String {
    let spelled = spell_digit_run(digits);
    let mut words: Vec<&str> = spelled.split(' ').collect();
    let Some(last) = words.pop() else { return spelled };
    let ordinal_last = cardinal_word_to_ordinal(last);
    if words.is_empty() {
        ordinal_last
    } else {
        format!("{} {}", words.join(" "), ordinal_last)
    }
}

/// Converts one cardinal number word to its ordinal form. Covers the
/// irregular cases (`one` -> `first`, `twelve` -> `twelfth`, `hundred` ->
/// `hundredth`, ...) and the `y` -> `ieth` tens rule (`twenty` ->
/// `twentieth`); anything else (`four`, `six`, `eleven`, ...) just gets a
/// plain `th` appended.
fn cardinal_word_to_ordinal(word: &str) -> String {
    match word {
        "zero" => "zeroth".to_owned(),
        "one" => "first".to_owned(),
        "two" => "second".to_owned(),
        "three" => "third".to_owned(),
        "five" => "fifth".to_owned(),
        "eight" => "eighth".to_owned(),
        "nine" => "ninth".to_owned(),
        "twelve" => "twelfth".to_owned(),
        "hundred" => "hundredth".to_owned(),
        "thousand" => "thousandth".to_owned(),
        _ => {
            if let Some(stem) = word.strip_suffix('y') {
                format!("{stem}ieth")
            } else {
                format!("{word}th")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pass 7: expand journal links — (AUGMENT,71279,) → "Augment 7 1 2 7 9"
// ---------------------------------------------------------------------------

/// Matches optional open bracket, 2+ uppercase letters, comma (optionally
/// followed by whitespace — real citations aren't always typed with the
/// digits glued directly to the comma), digits, comma, optional close
/// bracket. e.g. `(AUGMENT,71279,)`, `(AUGMENT, 81010,)`, or `<OAD,2237,>`
#[cfg(test)]
fn expand_journal_links(text: &str) -> String {
    flatten(&expand_journal_links_spanned(spanned_from_input(text)))
}

/// If a journal-link pattern starts at `i`, return (name, digits, end-index).
fn match_journal_link(chars: &[char], i: usize) -> Option<(String, String, usize)> {
    let name_start = if matches!(chars[i], '(' | '<') { i + 1 } else { i };
    let mut j = name_start;
    while j < chars.len() && chars[j].is_ascii_uppercase() { j += 1; }
    if j - name_start < 2 || j >= chars.len() || chars[j] != ',' { return None; }

    let mut digit_start = j + 1;
    while digit_start < chars.len() && chars[digit_start] == ' ' { digit_start += 1; }
    let mut k = digit_start;
    while k < chars.len() && chars[k].is_ascii_digit() { k += 1; }
    if k == digit_start || k >= chars.len() || chars[k] != ',' { return None; }

    let mut end = k + 1;
    if end < chars.len() && matches!(chars[end], ')' | '>') { end += 1; }

    Some((
        chars[name_start..j].iter().collect(),
        chars[digit_start..k].iter().collect(),
        end,
    ))
}

/// Span-aware version of `expand_journal_links` — same flush/expand shape
/// as the other multi-char expansions; the matched range covers the whole
/// link (brackets included) as one source span.
fn expand_journal_links_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_journal_links_chunk(&chunk)).collect()
}

fn expand_journal_links_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if let Some((name, digits, end)) = match_journal_link(&chars, i) {
            flush(&mut out, &chars, base, run_start, i);
            let spoken_name = process_alpha_token(&name);
            let spelled: Vec<&str> = digits.chars().map(digit_word).collect();
            let expanded = format!("{} {}", spoken_name, spelled.join(" "));
            out.push(Spanned { text: expanded, src: (base + i)..(base + end) });
            run_start = end;
            i = end;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 8b: expand a rate-unit slash ("cycles/sec" -> "cycles per second")
// ---------------------------------------------------------------------------

/// Rate-unit abbreviations recognized right after a `/` in a word/word rate
/// expression, mapped to their full singular spoken form. Both singular and
/// plural source spellings map to the same singular output — "per" always
/// takes a singular unit regardless of how the source wrote it.
fn rate_unit_full_name(word: &str) -> Option<&'static str> {
    Some(match word.to_ascii_lowercase().as_str() {
        "sec" | "secs" => "second",
        "min" | "mins" => "minute",
        "hr" | "hrs" => "hour",
        "day" | "days" => "day",
        "week" | "weeks" => "week",
        "month" | "months" => "month",
        "year" | "years" => "year",
        "decade" | "decades" => "decade",
        _ => return None,
    })
}

#[cfg(test)]
fn expand_rate_units(text: &str) -> String {
    flatten(&expand_rate_units_spanned(spanned_from_input(text)))
}

/// Span-aware version. Matches an alphabetic word run, optional whitespace,
/// `/`, optional whitespace, then a known rate unit (case-insensitive,
/// singular or plural) at a word boundary. The unit table is what keeps this
/// narrow — ordinary word/word pairs like "and/or", "his/her",
/// "document/file" never land on a table entry, so they're left untouched
/// without needing their own exclusion list. Requiring an actual alphabetic
/// run (not just a word boundary) immediately before the `/` also keeps this
/// pass off a DOI's digit-led segments ("10.1016/j.ijpsycho...") and
/// phonetic slash notation ("the fricative /s/", where nothing but
/// whitespace precedes the first `/`).
fn expand_rate_units_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_rate_units_chunk(&chunk)).collect()
}

/// A matched `word/unit` rate expression: the left word's original text,
/// the matched unit's full spoken form, and the char range (`word_start` to
/// `unit_end`, inclusive of the left word, slash, and unit) it spans.
struct RateMatch {
    word_start: usize,
    unit_end: usize,
    left_word: String,
    unit_full: &'static str,
}

/// If a `word/unit` rate expression starts at `word_start`, return the
/// match. See `RateMatch` for what's captured.
fn match_rate_unit(chars: &[char], word_start: usize) -> Option<RateMatch> {
    // Word boundary before the left word.
    if word_start > 0 && is_word_char(chars[word_start - 1]) { return None; }

    let mut i = word_start;
    while i < chars.len() && chars[i].is_ascii_alphabetic() { i += 1; }
    if i == word_start { return None; }
    let word_end = i;

    while i < chars.len() && chars[i].is_whitespace() { i += 1; }
    if chars.get(i) != Some(&'/') { return None; }
    i += 1;
    while i < chars.len() && chars[i].is_whitespace() { i += 1; }

    let unit_start = i;
    while i < chars.len() && chars[i].is_ascii_alphabetic() { i += 1; }
    if i == unit_start { return None; }
    let unit_end = i;
    if unit_end < chars.len() && is_word_char(chars[unit_end]) { return None; }

    let unit_word: String = chars[unit_start..unit_end].iter().collect();
    let unit_full = rate_unit_full_name(&unit_word)?;
    let left_word: String = chars[word_start..word_end].iter().collect();

    Some(RateMatch { word_start, unit_end, left_word, unit_full })
}

fn expand_rate_units_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if let Some(m) = match_rate_unit(&chars, i) {
            flush(&mut out, &chars, base, run_start, m.word_start);
            let expanded = format!("{} per {}", m.left_word, m.unit_full);
            out.push(Spanned { text: expanded, src: (base + m.word_start)..(base + m.unit_end) });
            run_start = m.unit_end;
            i = m.unit_end;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 9: expand US state postal abbreviations ("City, ST" pattern only)
// ---------------------------------------------------------------------------

fn state_full_name(abbr: &str) -> Option<&'static str> {
    Some(match abbr {
        "AL" => "Alabama", "AK" => "Alaska", "AZ" => "Arizona", "AR" => "Arkansas",
        "CA" => "California", "CO" => "Colorado", "CT" => "Connecticut", "DE" => "Delaware",
        "FL" => "Florida", "GA" => "Georgia", "HI" => "Hawaii", "ID" => "Idaho",
        "IL" => "Illinois", "IN" => "Indiana", "IA" => "Iowa", "KS" => "Kansas",
        "KY" => "Kentucky", "LA" => "Louisiana", "ME" => "Maine", "MD" => "Maryland",
        "MA" => "Massachusetts", "MI" => "Michigan", "MN" => "Minnesota", "MS" => "Mississippi",
        "MO" => "Missouri", "MT" => "Montana", "NE" => "Nebraska", "NV" => "Nevada",
        "NH" => "New Hampshire", "NJ" => "New Jersey", "NM" => "New Mexico", "NY" => "New York",
        "NC" => "North Carolina", "ND" => "North Dakota", "OH" => "Ohio", "OK" => "Oklahoma",
        "OR" => "Oregon", "PA" => "Pennsylvania", "RI" => "Rhode Island", "SC" => "South Carolina",
        "SD" => "South Dakota", "TN" => "Tennessee", "TX" => "Texas", "UT" => "Utah",
        "VT" => "Vermont", "VA" => "Virginia", "WA" => "Washington", "WV" => "West Virginia",
        "WI" => "Wisconsin", "WY" => "Wyoming", "DC" => "District of Columbia",
        _ => return None,
    })
}

/// Expand a two-letter state code only when it appears right after a comma
/// (the "City, ST" pattern), e.g. "Denver, CO" → "Denver, Colorado". Codes
/// elsewhere in the text (e.g. as ordinary words) are left untouched.
#[cfg(test)]
fn expand_state_abbrevs(text: &str) -> String {
    flatten(&expand_state_abbrevs_spanned(spanned_from_input(text)))
}

/// If a known "City, ST" state-code pattern starts at `i` (comma, one
/// whitespace char, a 2-letter code, word boundary), return the state's
/// full name and the matched span length. Two letter-case forms are
/// recognized:
/// - 2 uppercase letters (`", CO"`) — the standard postal code. Span is
///   always 4 (`,` + whitespace + 2 letters).
/// - A capitalized letter + lowercase letter, immediately followed by a
///   `.` (`", Ca."`) — the informal mixed-case abbreviation. The `.` is
///   only a lookahead disambiguator here (mixed case alone is too easily
///   an ordinary word); whether it's *also* consumed as part of the match
///   (span 5) or left in place (span 4) depends on what follows it —
///   sentence splitting already happened before normalization runs, so a
///   period followed by whitespace + a lowercase letter is this
///   abbreviation's own period, not a real sentence boundary the splitter
///   would have broken on (e.g. "San Francisco, Ca. is my home." stays one
///   sentence) — dropping it avoids planting a fake sentence break with a
///   lowercase continuation. Anything else after it (whitespace +
///   uppercase, other punctuation, or end of chunk) reads as genuinely
///   terminal, so it's left untouched instead.
fn match_state_abbrev(chars: &[char], i: usize) -> Option<(&'static str, usize)> {
    if chars[i] != ',' || i + 3 >= chars.len() { return None; }
    if !chars[i + 1].is_whitespace() { return None; }
    if preceded_by_lowercase_word(chars, i) { return None; }

    if chars[i + 2].is_ascii_uppercase() && chars[i + 3].is_ascii_uppercase() {
        if i + 4 < chars.len() && is_word_char(chars[i + 4]) { return None; }
        let code: String = chars[i + 2..i + 4].iter().collect();
        return state_full_name(&code).map(|name| (name, 4));
    }

    if chars[i + 2].is_ascii_uppercase() && chars[i + 3].is_ascii_lowercase()
        && chars.get(i + 4) == Some(&'.')
    {
        if i + 5 < chars.len() && is_word_char(chars[i + 5]) { return None; }
        let code: String = [chars[i + 2], chars[i + 3]].iter().map(|c| c.to_ascii_uppercase()).collect();
        let name = state_full_name(&code)?;

        let mid_sentence_continuation = chars.get(i + 5).is_some_and(|c| c.is_whitespace())
            && chars.get(i + 6).is_some_and(|c| c.is_ascii_lowercase());
        let len = if mid_sentence_continuation { 5 } else { 4 };
        return Some((name, len));
    }

    None
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// True when the word ending right before the comma at `i` starts with a
/// lowercase letter. A city name is capitalized ("Sioux Falls, SD"), so a
/// lowercase word there means the code is something else — e.g. "the mean,
/// SD, and outliers" is standard deviation, not South Dakota. When nothing
/// word-like sits right before the comma (start of chunk, punctuation),
/// there's no evidence either way and expansion is allowed.
fn preceded_by_lowercase_word(chars: &[char], i: usize) -> bool {
    let mut start = i;
    while start > 0 && is_word_char(chars[start - 1]) { start -= 1; }
    start < i && chars[start].is_lowercase()
}

/// Span-aware version of `expand_state_abbrevs`. The matched span (4 chars
/// for `,` + whitespace + 2-letter code, or 5 when the mixed-case + period
/// variant consumes its own trailing period — see `match_state_abbrev`) is
/// the source range for the expanded `, <Full Name>` chunk.
fn expand_state_abbrevs_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_state_abbrevs_chunk(&chunk)).collect()
}

fn expand_state_abbrevs_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if let Some((name, len)) = match_state_abbrev(&chars, i) {
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned { text: format!(", {name}"), src: (base + i)..(base + i + len) });
            run_start = i + len;
            i += len;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 6a: currency amounts — "£2333" -> "2333 pounds", "$45.50" -> "45
// dollars and 50 cents"
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_currency(text: &str) -> String {
    flatten(&expand_currency_spanned(spanned_from_input(text)))
}

/// Span-aware version. Same flush/expand shape as Pass 6e (cent sign), just
/// with the symbol leading the digit run instead of trailing it, and an
/// optional ".NN" fractional part folded into the same match. The matched
/// range covers the symbol, whole-number digits, and (if present) the "."
/// plus exactly two fractional digits, as one source span.
fn expand_currency_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_currency_chunk(&chunk)).collect()
}

fn expand_currency_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    fn spell_whole(digits: &str) -> String {
        spell_bare_digit_group(digits).unwrap_or_else(|| {
            digits.chars().map(digit_word).collect::<Vec<_>>().join(" ")
        })
    }

    while i < chars.len() {
        let symbol_word = match chars[i] {
            '£' => Some(("pound", "pounds")),
            '$' => Some(("dollar", "dollars")),
            _ => None,
        };

        let Some((unit_singular, unit_plural)) = symbol_word else { i += 1; continue; };
        // Same word-boundary check as the cent-sign pass: a symbol glued
        // onto an identifier is left alone rather than guessed at.
        if i > 0 && chars[i - 1].is_alphanumeric() { i += 1; continue; }

        let digit_start = i + 1;
        let mut j = digit_start;
        while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
        if j == digit_start { i += 1; continue; }

        // Extend over comma-grouped digits ("37,000"), same grouping rule
        // as Pass 8's expand_comma_numbers_chunk: each group after the
        // first comma must be exactly 3 digits.
        while j < chars.len() && chars[j] == ','
            && j + 3 < chars.len()
            && chars[j + 1..j + 4].iter().all(|c| c.is_ascii_digit())
            && (j + 4 == chars.len() || !chars[j + 4].is_ascii_digit())
        {
            j += 4;
        }

        let whole_digits: String = chars[digit_start..j].iter().filter(|c| **c != ',').collect();
        let whole_spelled = if chars[digit_start..j].contains(&',') {
            number_to_words(whole_digits.parse().unwrap_or(0))
        } else {
            spell_whole(&whole_digits)
        };

        // A "." immediately followed by exactly two digits is the cents
        // part of the amount — anything else (one digit, three+ digits, or
        // no dot at all) leaves only the whole-number part claimed here,
        // same as Pass 4's "leave the rest for later" fallback.
        let mut end = j;
        let mut cents_digits: Option<String> = None;
        if j < chars.len() && chars[j] == '.' {
            let dec_start = j + 1;
            let mut k = dec_start;
            while k < chars.len() && chars[k].is_ascii_digit() { k += 1; }
            if k - dec_start == 2 {
                cents_digits = Some(chars[dec_start..k].iter().collect());
                end = k;
            }
        }

        let whole_val: u64 = whole_digits.parse().unwrap_or(0);
        let unit = if whole_val == 1 { unit_singular } else { unit_plural };

        let text = match cents_digits {
            Some(cents) if cents != "00" => {
                let cents_val: u32 = cents.parse().unwrap_or(0);
                let cents_word = if cents_val == 1 { "cent" } else { "cents" };
                format!(
                    "{whole_spelled} {unit} and {} {cents_word}",
                    number_to_words(cents_val as u64)
                )
            }
            _ => format!("{whole_spelled} {unit}"),
        };

        flush(&mut out, &chars, base, run_start, i);
        out.push(Spanned { text, src: (base + i)..(base + end) });
        run_start = end;
        i = end;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 6b: number sign
// ---------------------------------------------------------------------------

#[cfg(test)]
fn replace_number_sign(text: &str) -> String {
    flatten(&replace_number_sign_spanned(spanned_from_input(text)))
}

/// Span-aware version. Each matched `#` (a single source char) expands to
/// `"number "` — same flush/expand shape as `replace_identifier_dots_chunk`,
/// just with a different trigger character and no point-vs-dot branch.
fn replace_number_sign_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| replace_number_sign_chunk(&chunk)).collect()
}

fn replace_number_sign_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    for i in 0..chars.len() {
        // Only "#" directly followed by a digit counts as "number" — a
        // hashtag ("#quaker") or "C#" don't have a digit right there, and
        // are left untouched.
        if chars[i] == '#' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned { text: "number ".to_owned(), src: (base + i)..(base + i + 1) });
            run_start = i + 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 6c: identifier dots and decimal points
// ---------------------------------------------------------------------------

#[cfg(test)]
fn replace_identifier_dots(text: &str) -> String {
    flatten(&replace_identifier_dots_spanned(spanned_from_input(text)))
}

#[cfg(test)]
fn replace_ellipsis(text: &str) -> String {
    flatten(&replace_ellipsis_spanned(spanned_from_input(text)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bible_ref_abbrev_with_period() {
        assert_eq!(expand_bible_refs("Mt. 5:30"), "Matthew chapter five verse thirty");
    }

    #[test]
    fn bible_ref_full_book_name() {
        assert_eq!(expand_bible_refs("Matthew 5:30"), "Matthew chapter five verse thirty");
    }

    #[test]
    fn bible_ref_psalms_abbrev_no_chapter_word() {
        assert_eq!(expand_bible_refs("Ps. 73:21"), "Psalm seventy three verse twenty one");
    }

    #[test]
    fn bible_ref_psalms_full_name() {
        assert_eq!(expand_bible_refs("Psalm 73:21"), "Psalm seventy three verse twenty one");
    }

    #[test]
    fn bible_ref_verse_range_same_chapter() {
        assert_eq!(expand_bible_refs("Ps. 40:7-8"), "Psalm forty verse seven to eight");
    }

    #[test]
    fn bible_ref_numbered_book() {
        assert_eq!(expand_bible_refs("1 Cor. 13:4"), "First Corinthians chapter thirteen verse four");
    }

    #[test]
    fn bible_ref_in_sentence() {
        assert_eq!(
            expand_bible_refs("As it says in Mt. 5:30, turn the other cheek."),
            "As it says in Matthew chapter five verse thirty, turn the other cheek."
        );
    }

    #[test]
    fn bible_ref_no_match_without_period_on_abbrev() {
        // "Mt" with no trailing period is not treated as an abbreviation.
        assert_eq!(expand_bible_refs("Mt 5:30"), "Mt 5:30");
    }

    #[test]
    fn bible_ref_no_match_for_ordinary_colon() {
        assert_eq!(expand_bible_refs("The meeting is at 5:30"), "The meeting is at 5:30");
    }

    #[test]
    fn roman_numerals_converted() {
        assert_eq!(process_alpha_token("II"),    "two");
        assert_eq!(process_alpha_token("III"),   "three");
        assert_eq!(process_alpha_token("IV"),    "four");
        assert_eq!(process_alpha_token("VIII"),  "eight");
        assert_eq!(process_alpha_token("IX"),    "nine");
        assert_eq!(process_alpha_token("XIV"),   "fourteen");
        assert_eq!(process_alpha_token("XIX"),   "nineteen");
        assert_eq!(process_alpha_token("XX"),    "twenty");
        assert_eq!(process_alpha_token("XXI"),   "twenty one");
        assert_eq!(process_alpha_token("XLII"),  "forty two");
        assert_eq!(process_alpha_token("XVIII"), "eighteen");
        assert_eq!(process_alpha_token("MCMXCIX"), "one thousand nine hundred ninety nine");
    }

    #[test]
    #[ignore = "lowercase Roman numerals disabled: indistinguishable from \
                placeholder strings like 'xxx'/'yy' in authorship.txt; \
                would need per-document overrides to do both correctly"]
    fn roman_lowercase_converted() {
        assert_eq!(process_alpha_token("ii"),    "two");
        assert_eq!(process_alpha_token("iii"),   "three");
        assert_eq!(process_alpha_token("iv"),    "four");
        assert_eq!(process_alpha_token("viii"),  "eight");
        assert_eq!(process_alpha_token("xiv"),   "fourteen");
        assert_eq!(process_alpha_token("xcix"),  "ninety nine");
    }

    #[test]
    fn lowercase_placeholder_strings_unchanged() {
        // "xxx", "yy" etc. as Journal-item placeholders (authorship.txt) must
        // not be read as Roman numerals.
        assert_eq!(process_alpha_token("xxx"), "xxx");
        assert_eq!(process_alpha_token("yy"),  "yy");
        assert_eq!(process_alpha_token("mix"), "mix");
    }

    #[test]
    fn roman_single_chars_not_converted() {
        // Single Roman-numeral chars are too ambiguous — left to existing all-caps logic.
        assert_eq!(process_alpha_token("I"), "I");
        assert_eq!(process_alpha_token("V"), "V");
        assert_eq!(process_alpha_token("X"), "X");
        assert_eq!(process_alpha_token("i"), "i");
    }

    #[test]
    fn non_roman_allcaps_unchanged() {
        // These fail the canonical re-encoding check and fall through to all-caps handling.
        assert_eq!(process_alpha_token("CIVIL"),  "Civil");
        assert_eq!(process_alpha_token("MILL"),   "Mill");
        // MIX is a genuine Roman numeral (1009 = M + IX) — converted, not lowercased.
        assert_eq!(process_alpha_token("MIX"),    "one thousand nine");
    }

    #[test]
    fn two_letter_initials_not_roman() {
        // Author initials in a citation ("Oshinsky ML.") parse as Roman
        // numerals but must be spelled out.
        assert_eq!(process_alpha_token("ML"), "M L");
        assert_eq!(process_alpha_token("MD"), "M D");
        assert_eq!(process_alpha_token("DC"), "D C");
        assert_eq!(process_alpha_token("XX"), "twenty");
        assert_eq!(process_alpha_token("IV"), "four");
    }

    #[test]
    fn roman_possessive() {
        assert_eq!(process_alpha_token("VIII's"), "eight's");
    }

    #[test]
    fn number_to_words_spot_checks() {
        assert_eq!(number_to_words(1),      "one");
        assert_eq!(number_to_words(14),     "fourteen");
        assert_eq!(number_to_words(20),     "twenty");
        assert_eq!(number_to_words(21),     "twenty one");
        assert_eq!(number_to_words(100),    "one hundred");
        assert_eq!(number_to_words(1999),   "one thousand nine hundred ninety nine");
        assert_eq!(number_to_words(2000),   "two thousand");
        assert_eq!(number_to_words(100000), "one hundred thousand");
    }

    #[test]
    fn leading_zero_numbers_spelled_as_words() {
        assert_eq!(process_token("0609", &HashMap::new()), "zero six zero nine");
        assert_eq!(process_token("069",  &HashMap::new()), "zero six nine");
        assert_eq!(process_token("012",  &HashMap::new()), "zero one two");
        // single "0" is not an "ID" — leave alone.
        assert_eq!(process_token("0",    &HashMap::new()), "0");
    }

    #[test]
    fn short_quotes_stripped() {
        assert_eq!(strip_short_quotes(r#""foo bar""#), "foo bar");
        assert_eq!(strip_short_quotes(r#""one two three four five""#), "one two three four five");
        // 6 words — keep quotes
        assert_eq!(strip_short_quotes(r#""one two three four five six""#),
                   r#""one two three four five six""#);
        assert_eq!(strip_short_quotes(r#"He said "hello" and left"#), "He said hello and left");
    }

    #[test]
    fn lowercase_sentence_start_is_capitalized() {
        // Unconditional — this text is only ever heard via TTS, never read,
        // so there's no reason to preserve a lowercase sentence-opener
        // regardless of whether it's original source text or a pass's
        // output (e.g. a single-word override used verbatim).
        assert_eq!(capitalize_sentence_start("sequel is a language"), "Sequel is a language");
    }

    #[test]
    fn already_capitalized_sentence_start_is_unchanged() {
        assert_eq!(capitalize_sentence_start("Sequel is a language"), "Sequel is a language");
    }

    #[test]
    fn non_alphabetic_sentence_start_is_unchanged() {
        assert_eq!(capitalize_sentence_start("42 is the answer"), "42 is the answer");
    }

    #[test]
    fn override_at_sentence_start_is_capitalized() {
        // "sql" -> "sequel" is used verbatim (lowercase) regardless of
        // position; at the very start of a sentence that reads oddly and
        // has caused TTS artifacts, so the final capitalize-sentence-start
        // pass fixes it up.
        let mut overrides = HashMap::new();
        overrides.insert("sql".to_string(), "sequel".to_string());
        let chunks = tokenize_spanned(spanned_from_input("SQL is a language"), &overrides);
        assert_eq!(flatten(&chunks), "sequel is a language");
        let chunks = capitalize_sentence_start_spanned(chunks);
        assert_eq!(flatten(&chunks), "Sequel is a language");
    }

    #[test]
    fn identifier_dots_replaced() {
        assert_eq!(replace_identifier_dots("4b.l"),   "4b dot l");
        assert_eq!(replace_identifier_dots("4b.dt"),  "4b dot dt");
        assert_eq!(replace_identifier_dots("Ref.l"),  "Ref dot l");
        // digits on both sides read as a decimal point, not "dot"
        assert_eq!(replace_identifier_dots("3.14"),   "3 point 14");
        // standalone: space before dot, alphanumeric after — also replaced
        assert_eq!(replace_identifier_dots(" .l "),   "  dot l ");
        // dot at end of sentence — unchanged (nothing alphanumeric after)
        assert_eq!(replace_identifier_dots("end."),   "end.");
        assert_eq!(replace_identifier_dots(". foo"),  ". foo");
        // ellipsis — middle dots preceded by ".", not replaced
        assert_eq!(replace_identifier_dots("..."),    "...");
        assert_eq!(replace_identifier_dots("foo...bar"), "foo...bar");
    }

    #[test]
    fn at_sign_read_as_at() {
        assert_eq!(replace_identifier_dots("support@example.us"), "support at example dot us");
        assert_eq!(replace_identifier_dots("follow @exampleus"), "follow at exampleus");
        assert_eq!(replace_identifier_dots("(@exampleus)"), "(at exampleus)");
        // nothing alphanumeric after — unchanged
        assert_eq!(replace_identifier_dots("a lone @ sign"), "a lone @ sign");
        assert_eq!(replace_identifier_dots("foo@"), "foo@");
        assert_eq!(
            normalize("Questions or requests about your data go to support@example.us."),
            "Questions or requests about your data go to support at example dot us."
        );
        assert_eq!(normalize("@exampleus posted it."), "At exampleus posted it.");
    }

    #[test]
    fn decimal_dots_read_as_point_through_full_pipeline() {
        // Regression: "26.44" used to normalize to "twenty six.forty four"
        // — the "." survived bare and glued to "forty" with no space, since
        // Pass 6d/7 spelled the flanking digits into separate non-raw
        // chunks before the old, later-running dot pass ever got a chance
        // to see across that boundary. Reordering the dot pass to run
        // before digit-spelling (Pass 6c) fixes this generally, not just
        // for this one input.
        assert_eq!(normalize("pi is 3.14"), "Pi is three point fourteen");
    }

    #[test]
    fn identifier_dot_before_digit_run_also_fixed_by_reorder() {
        // Same underlying bug, mixed-identifier flavor: a dot immediately
        // followed by a digit run (not preceded by one) also used to strand
        // bare — "Ref.1974" normalized to "Ref.nineteen seventy four".
        assert_eq!(normalize("Ref.1974"), "Ref dot nineteen seventy four");
    }

    #[test]
    fn number_sign_replaced() {
        assert_eq!(replace_number_sign("#26"),      "number 26");
        assert_eq!(replace_number_sign("#5"),       "number 5");
        // not followed by a digit — left alone
        assert_eq!(replace_number_sign("#quaker"),  "#quaker");
        assert_eq!(replace_number_sign("C#"),       "C#");
        assert_eq!(replace_number_sign("no hash"),  "no hash");
    }

    #[test]
    fn number_sign_before_decimal_reference_through_full_pipeline() {
        // Regression: "#26.44" (a paragraph.item citation, recurring
        // throughout the Quaker Faith & Practice document) left the "#"
        // bare and glued directly onto "twenty" — same class of artifact
        // as the leading em-dash and stranded-dot bugs, and equally
        // invisible to forced alignment (not a letter, no space around it).
        // Must run before Pass 6d/7 spell the digits out, same reasoning as
        // Pass 6c's dot handling.
        assert_eq!(
            normalize("Practice, 1995, #26.44 Individual growth"),
            "Practice, nineteen ninety five, number twenty six point forty four Individual growth"
        );
    }

    #[test]
    fn hashtag_and_csharp_unaffected_by_number_sign_pass() {
        assert_eq!(normalize("See #quaker for details"), "See #quaker for details");
        assert_eq!(normalize("Written in C#"), "Written in C#");
    }

    #[test]
    fn item_numbers_spelled_digit_by_digit() {
        assert_eq!(spell_item_numbers("Item 71279"), "Item seven one two seven nine");
        assert_eq!(spell_item_numbers("Item 1000"),  "Item one zero zero zero");
        // fewer than 4 digits — left alone
        assert_eq!(spell_item_numbers("Item 42"),    "Item 42");
        // not at word boundary — left alone
        assert_eq!(spell_item_numbers("Items 71279"), "Items 71279");
    }

    #[test]
    fn ellipsis_stripped() {
        // Both sides are words — a separating space is inserted so they
        // don't glue together, whether or not the source had one.
        assert_eq!(replace_ellipsis("foo...bar"),        "foo bar");
        assert_eq!(replace_ellipsis("foo... bar"),        "foo bar");
        assert_eq!(replace_ellipsis("foo \u{2026} bar"),  "foo bar");
        assert_eq!(replace_ellipsis("foo\u{2026}bar"),   "foo bar");
        // Punctuation follows — attaches directly, no space inserted.
        assert_eq!(replace_ellipsis("foo...,bar"),       "foo,bar");
        assert_eq!(replace_ellipsis(r#"foo..." bar"#),   r#"foo" bar"#);
        assert_eq!(replace_ellipsis("foo\u{2026},bar"),  "foo,bar");
        // leading space consumed so orphaned ` ,` doesn't remain
        assert_eq!(replace_ellipsis("every ..., and"),   "every, and");
        assert_eq!(replace_ellipsis("every \u{2026}, and"), "every, and");
        // Sentence-initial ellipsis — no leading space introduced.
        assert_eq!(replace_ellipsis("...and so it began."), "and so it began.");
        assert_eq!(replace_ellipsis("\u{2026}and so it began."), "and so it began.");
    }

    #[test]
    fn ellipsis_runs_through_full_pipeline_even_after_tokenizing() {
        // Regression: Pass 10's tokenizer used to split "..." into three
        // separate 1-char chunks before this pass ran, so a literal
        // 3-char-substring match could never fire again — only the single
        // codepoint "…" form was ever actually stripped. Moving this pass
        // to run before tokenization (Pass 5b) fixes it for both forms.
        assert_eq!(
            normalize("He paused... then continued."),
            "He paused then continued."
        );
        assert_eq!(
            normalize("He paused\u{2026}then continued."),
            "He paused then continued."
        );
    }

    #[test]
    fn spaced_ellipsis_stripped() {
        // Regression: a real document has ellipsis typed as individually
        // spaced periods ("right now. . . . A concern") instead of a
        // contiguous "..." or the "…" codepoint — neither of which the
        // match previously recognized at all, leaving every period to
        // reach the TTS backend as literal, unhandled punctuation.
        assert_eq!(
            replace_ellipsis("right now. . . . A concern"),
            "right now A concern",
        );
        assert_eq!(
            replace_ellipsis("right now . . . a concern"),
            "right now a concern",
        );
        // Fewer than 3 dots isn't an ellipsis — leaves real abbreviation-like
        // patterns and ordinary sentence boundaries alone.
        assert_eq!(replace_ellipsis("the U. S. Government"), "the U. S. Government");
        assert_eq!(replace_ellipsis("Wait. Next sentence."), "Wait. Next sentence.");
    }

    #[test]
    fn comma_grouped_numbers_expanded() {
        assert_eq!(expand_comma_numbers("2,000 characters"), "two thousand characters");
        assert_eq!(expand_comma_numbers("100,000 items"), "one hundred thousand items");
        // not a thousands grouping (only 2 digits after comma): "4b" is
        // letter-adjacent so stays alone; bare "12" gets digit-group spelled.
        assert_eq!(expand_comma_numbers("(4b,12)"), "(4b,twelve)");
    }

    #[test]
    fn three_digit_numbers_digit_group_spelled() {
        assert_eq!(three_digit_group_words("560"), "five sixty");
        assert_eq!(three_digit_group_words("501"), "five oh one");
        assert_eq!(three_digit_group_words("500"), "five hundred");
        assert_eq!(three_digit_group_words("100"), "one hundred");
        assert_eq!(three_digit_group_words("111"), "one eleven");
    }

    #[test]
    fn bare_three_digit_numbers_expanded_in_context() {
        assert_eq!(expand_comma_numbers("I have 560 dollars"), "I have five sixty dollars");
        assert_eq!(expand_comma_numbers("apartment 501"), "apartment five oh one");
        assert_eq!(expand_comma_numbers("500 years"), "five hundred years");
    }

    #[test]
    fn four_digit_numbers_digit_group_spelled() {
        assert_eq!(four_digit_group_words("1976"), "nineteen seventy six");
        assert_eq!(four_digit_group_words("2010"), "twenty ten");
        assert_eq!(four_digit_group_words("2005"), "twenty oh five");
        assert_eq!(four_digit_group_words("1905"), "nineteen oh five");
        assert_eq!(four_digit_group_words("1900"), "nineteen hundred");
        assert_eq!(four_digit_group_words("2300"), "twenty three hundred");
        assert_eq!(four_digit_group_words("2000"), "two thousand");
        assert_eq!(four_digit_group_words("1000"), "one thousand");
        assert_eq!(four_digit_group_words("3000"), "three thousand");
    }

    #[test]
    fn bare_four_digit_numbers_expanded_in_context() {
        assert_eq!(expand_comma_numbers("born in 1976"), "born in nineteen seventy six");
        assert_eq!(expand_comma_numbers("item 2237"), "item twenty two thirty seven");
    }

    #[test]
    fn bare_one_and_two_digit_numbers_expanded() {
        assert_eq!(expand_comma_numbers("only 12 left"), "only twelve left");
        assert_eq!(expand_comma_numbers("I had 3 cats"), "I had three cats");
        assert_eq!(expand_comma_numbers("page 0"), "page zero");
        assert_eq!(expand_comma_numbers("apartment 42"), "apartment forty two");
    }

    #[test]
    fn bare_digit_groups_excluded_when_touching_a_letter() {
        // Alphanumeric IDs aren't bare numbers — split_alphanumeric (later
        // in the pipeline) handles spacing these out, not this rule.
        assert_eq!(expand_comma_numbers("(4b,12)"), "(4b,twelve)");
        assert_eq!(expand_comma_numbers("v2 release"), "v2 release");
        assert_eq!(expand_comma_numbers("room 14B"), "room 14B");
    }

    // A `-` directly against a digit is a negative sign, spelled as
    // "negative" — not silently dropped (Pass 10's generic hyphen-to-space
    // rule would otherwise blank it, leaving "-1" indistinguishable from
    // "1" once spelled out).
    #[test]
    fn negative_numbers_keep_their_sign() {
        assert_eq!(expand_comma_numbers("-1, -2"), "negative one, negative two");
        assert_eq!(expand_comma_numbers("(-1)"), "(negative one)");
        assert_eq!(expand_comma_numbers("-2,000 dollars"), "negative two thousand dollars");
    }

    // A digit-hyphen-digit run with something alphanumeric right before the
    // hyphen isn't a negative sign — a subtraction/range like "5-3" (any
    // genuine year range was already consumed by Pass 4 before this pass
    // runs) or a hyphenated code like "F30602-76" stays untouched here.
    #[test]
    fn hyphen_after_alphanumeric_is_not_treated_as_negative() {
        assert_eq!(expand_comma_numbers("5-3"), "five-three");
    }

    #[test]
    fn bare_digit_groups_excluded_cases_unchanged() {
        // Leading zero (length > 1) — handled elsewhere (digit-by-digit ID
        // spelling): a lone "0" still spells out as "zero" via this rule.
        assert_eq!(expand_comma_numbers("code 012"), "code 012");
        assert_eq!(expand_comma_numbers("code 0123"), "code 0123");
        // Comma-grouped takes precedence over the bare-digit-group rule.
        assert_eq!(expand_comma_numbers("2,560 total"), "two thousand five hundred sixty total");
    }

    #[test]
    fn bare_five_plus_digit_numbers_spelled_digit_by_digit() {
        // Regression: a bare 5+ digit number (spell_bare_digit_group only
        // covers 1-4 digits) used to pass through completely unspoken —
        // invisible to forced alignment too, since it has no letters in
        // its vocabulary. Falls back to spelling each digit individually,
        // the same fallback already used elsewhere in this file (Pass 3's
        // <Ref-N> tags, Pass 3b's footnote markers, Pass 4's numeric
        // ranges) whenever a number is too long for the digit-group
        // convention. Found via a real document's bare ISBN component,
        // "ISBN 0-87574-297-3" (the "87574" segment).
        assert_eq!(expand_comma_numbers("serial 12345"), "serial one two three four five");
        assert_eq!(expand_comma_numbers("87574"), "eight seven five seven four");
        // A leading zero on a 5+ digit run is unaffected by this fallback
        // either way — digit-by-digit spelling doesn't care about it.
        assert_eq!(expand_comma_numbers("012345"), "zero one two three four five");
        // Still excluded when touching a letter — split_alphanumeric
        // (later in the pipeline) handles that case instead.
        assert_eq!(expand_comma_numbers("room 12345B"), "room 12345B");
    }

    #[test]
    fn long_allcaps_title_cased() {
        assert_eq!(process_alpha_token("AUGMENT"), "Augment");
        assert_eq!(process_alpha_token("IMPORTANT"), "Important");
    }

    #[test]
    fn short_allcaps_spelled_out() {
        assert_eq!(process_alpha_token("UIS"), "U I S");
        assert_eq!(process_alpha_token("TTS"), "T T S");
        assert_eq!(process_alpha_token("DNA"), "D N A");
    }

    #[test]
    fn short_allcaps_common_word_title_cased_not_spelled_out() {
        // Regression: a real ALL-CAPS TITLE HEADING style ("THE END", "TO
        // BE OR NOT TO BE") hit the same ≤3-letter acronym-spelling branch
        // as a real acronym like "TTS" — these common function words were
        // misread as acronyms ("T H E", "F O R"). A prior fix added
        // case-insensitive override table entries for a few of these
        // words directly, but that forced a normal sentence-initial "The"/
        // "To"/"For"/"Not" to lowercase too (overrides apply regardless of
        // the token's actual case), which is believed to have caused
        // audible artifacts at those sentence boundaries. Fixed here
        // instead, gated on `all_caps` so a normal mixed-case token never
        // reaches this branch at all.
        assert_eq!(process_alpha_token("THE"), "The");
        assert_eq!(process_alpha_token("TO"), "To");
        assert_eq!(process_alpha_token("FOR"), "For");
        assert_eq!(process_alpha_token("IN"), "In");
        assert_eq!(process_alpha_token("OF"), "Of");
        assert_eq!(process_alpha_token("AND"), "And");
        assert_eq!(process_alpha_token("NOT"), "Not");
        assert_eq!(process_alpha_token("BE"), "Be");
        // A real acronym of the same length is unaffected.
        assert_eq!(process_alpha_token("TTS"), "T T S");
        // Normal mixed-case usage was never broken in the first place —
        // confirms the fix doesn't touch sentence-initial capitalization.
        assert_eq!(
            normalize("The cat sat on the mat."),
            "The cat sat on the mat.",
        );
        assert_eq!(
            normalize("Not everyone agreed with the plan."),
            "Not everyone agreed with the plan.",
        );
    }

    #[test]
    fn four_char_allcaps_title_cased() {
        assert_eq!(process_alpha_token("NASA"), "Nasa");
        assert_eq!(process_alpha_token("HTTP"), "Http");
    }

    #[test]
    fn mixed_case_untouched() {
        assert_eq!(process_alpha_token("Hello"), "Hello");
        assert_eq!(process_alpha_token("iPhone"), "iPhone");
    }

    #[test]
    fn possessive_allcaps_lowercased() {
        assert_eq!(process_alpha_token("AUGMENT's"), "Augment's");
        assert_eq!(process_alpha_token("TTS's"), "T T S's");
    }

    #[test]
    fn alphanumeric_split() {
        // Digit runs are spelled out, not left raw — raw digits are hard
        // for TTS and untimeable by forced alignment (see split_alphanumeric).
        assert_eq!(split_alphanumeric("1a"),    "one a");
        assert_eq!(split_alphanumeric("4c2"),   "four c two");
        assert_eq!(split_alphanumeric("4C2"),   "four C two");
        assert_eq!(split_alphanumeric("12ab34"), "twelve ab thirty four");
        // Memory-size shorthand: the motivating case.
        assert_eq!(split_alphanumeric("512K"),  "five twelve K");
        assert_eq!(split_alphanumeric("128K"),  "one twenty eight K");
        // Longer/leading-zero digit runs fall back to digit-by-digit.
        assert_eq!(split_alphanumeric("007b"),  "zero zero seven b");
    }

    #[test]
    fn tag_expansion() {
        assert_eq!(try_expand_tag("Ref-3"),   Some("Ref three".into()));
        assert_eq!(try_expand_tag("Ref-12"),  Some("Ref twelve".into()));
        assert_eq!(try_expand_tag("Fig-1"),   Some("Figure one".into()));
        assert_eq!(try_expand_tag("Table-2"), Some("Table two".into()));
        assert_eq!(try_expand_tag("em"),      None);
        assert_eq!(try_expand_tag("Foo-3"),   Some("Foo three".into()));
        // 4-digit tag uses the same digit-group convention as Pass 8/4.
        assert_eq!(try_expand_tag("Ref-1976"), Some("Ref nineteen seventy six".into()));
        // 5+ digits aren't covered by `spell_bare_digit_group` — left bare.
        assert_eq!(try_expand_tag("Ref-12345"), Some("Ref 12345".into()));
    }

    #[test]
    fn numeric_range_expansion() {
        assert_eq!(expand_numeric_ranges("1976-77"),
            "nineteen seventy six to seventy seven");
        assert_eq!(expand_numeric_ranges("1976-1977"),
            "nineteen seventy six to nineteen seventy seven");
        assert_eq!(expand_numeric_ranges("in 1976-77 we"),
            "in nineteen seventy six to seventy seven we");
        assert_eq!(expand_numeric_ranges("pages 10-20"),
            "pages ten to twenty");
        // Regression: both halves must be spelled, not just the hyphen
        // replaced — bare digits here would be invisible to forced
        // alignment (no letters), dropping a selected passage spanning them
        // entirely from the aligned word list rather than just mismatching.
        assert_eq!(expand_numeric_ranges("in Winchester, MA in 1973-76"),
            "in Winchester, MA in nineteen seventy three to seventy six");
    }

    #[test]
    fn numeric_range_en_dash() {
        // En-dash as range separator — common in technical writing ("300–3.4 kHz").
        assert_eq!(expand_numeric_ranges("300\u{2013}3.4 kHz"),
            "three hundred to three point four kHz");
        assert_eq!(expand_numeric_ranges("5\u{2013}8 kHz"),
            "five to eight kHz");
        assert_eq!(expand_numeric_ranges("5.6\u{2013}8"),
            "five point six to eight");
    }

    #[test]
    fn numeric_range_comma_grouped_halves() {
        // A comma-grouped half used to cut the range short at the comma
        // ("100-12" + ",000" read digit by digit).
        assert_eq!(expand_numeric_ranges("100-12,000 Hz"),
            "one hundred to twelve thousand Hz");
        assert_eq!(expand_numeric_ranges("1,000-2,000"),
            "one thousand to two thousand");
        assert_eq!(expand_numeric_ranges("1,000\u{2013}1,500"),
            "one thousand to one thousand five hundred");
        // Not a valid comma group (needs exactly three digits): the comma
        // ends the second half.
        assert_eq!(expand_numeric_ranges("3-4,5"), "three to four,5");
        // Full pipeline, including the Hz unit expansion.
        assert_eq!(normalize("100-12,000 Hz"), "One hundred to twelve thousand Hertz");
    }

    #[test]
    fn numeric_range_spaced_hyphen() {
        // "4 - 5" with spaces around the ASCII hyphen.
        assert_eq!(expand_numeric_ranges("4 - 5"), "four to five");
        assert_eq!(expand_numeric_ranges("pages 4 - 5 kHz"), "pages four to five kHz");
        // Unspaced ASCII hyphen (existing behaviour preserved).
        assert_eq!(expand_numeric_ranges("4-5"), "four to five");
    }

    #[test]
    fn numeric_range_ascii_hyphen_requires_symmetric_spacing() {
        // Regression: an ASCII hyphen with a space on only *one* side is
        // ambiguous with a spaced-out negative number ("10 -2" reads as
        // "ten, negative two", not "ten to two") — only symmetric spacing
        // (both sides or neither) counts as a range separator for the
        // ASCII hyphen. En-dash has no such ambiguity (English prose
        // doesn't use it for negation), so it stays spaced-independently.
        assert_eq!(expand_numeric_ranges("it was 10 -2 degrees"), "it was 10 -2 degrees");
        assert_eq!(expand_numeric_ranges("y = 10 -2"), "y = 10 -2");
        assert_eq!(expand_numeric_ranges("10- 2"), "10- 2");
        // En-dash: either side may be spaced independently.
        assert_eq!(expand_numeric_ranges("300\u{2013} 3.4 kHz"), "three hundred to three point four kHz");
        assert_eq!(expand_numeric_ranges("300 \u{2013}3.4 kHz"), "three hundred to three point four kHz");
    }

    #[test]
    fn numeric_range_three_plus_segment_chain_is_not_a_range() {
        // Regression: a real document's ISBN, "ISBN 0-87574-297-3.", used
        // to have its tail pair misread as a range ("...two hundred ninety
        // seven to three") since the pass only ever looked at one hyphen
        // pair at a time. A 3+-segment hyphen chain (ISBN, phone number,
        // catalog ID, version string) isn't a range at all — left alone
        // entirely, not even a sub-pair within it.
        assert_eq!(expand_numeric_ranges("0-87574-297-3"), "0-87574-297-3");
        assert_eq!(expand_numeric_ranges("10-5-3"), "10-5-3");
        // Exactly two segments is still a genuine range.
        assert_eq!(expand_numeric_ranges("pp. 99-108"), "pp. ninety nine to one oh eight");
    }

    #[test]
    fn numeric_range_leading_zero_half_is_not_a_range() {
        // Regression: a real document's Library of Congress catalog card
        // number, "Catalog Card # 91-061844.", used to be misread as a
        // range ("...ninety one to zero six one eight four four") — a
        // leading zero on either half (length > 1) signals an ID/serial
        // number, not a magnitude, same convention Pass 8/Pass 10 already
        // use for bare numbers elsewhere in this file.
        assert_eq!(expand_numeric_ranges("91-061844"), "91-061844");
        assert_eq!(expand_numeric_ranges("061844-91"), "061844-91");
        // A genuine range with no leading zero on either side is unaffected.
        assert_eq!(expand_numeric_ranges("91-108"), "ninety one to one oh eight");
    }

    #[test]
    fn numeric_range_decimal_both_sides() {
        assert_eq!(expand_numeric_ranges("3.4-8.2"), "three point four to eight point two");
    }

    #[test]
    fn ratio_expands_to_cardinals() {
        assert_eq!(expand_ratios("2:1"), "two to one");
        assert_eq!(expand_ratios("16:9"), "sixteen to nine");
        assert_eq!(expand_ratios("100:1"), "one hundred to one");
        assert_eq!(expand_ratios("1,000:1"), "one thousand to one");
        assert_eq!(expand_ratios("1990:1"), "one thousand nine hundred ninety to one");
        assert_eq!(expand_ratios("3:200"), "three to two hundred");
        assert_eq!(expand_ratios("12:5"), "twelve to five");
    }

    #[test]
    fn ratio_decimals() {
        assert_eq!(expand_ratios("1.5:1"), "one point five to one");
        assert_eq!(expand_ratios("1:0.25"), "one to zero point twenty five");
    }

    #[test]
    fn ratio_in_context() {
        assert_eq!(
            normalize("This corresponds to a ratio of intensities of 1,000,000,000,000:1"),
            "This corresponds to a ratio of intensities of one trillion to one",
        );
        assert_eq!(normalize("a 16:9 screen."), "A sixteen to nine screen.");
    }

    #[test]
    fn ratio_leaves_clock_times_alone() {
        assert_eq!(expand_ratios("3:15"), "3:15");
        assert_eq!(expand_ratios("9:00"), "9:00");
        assert_eq!(expand_ratios("11:45 today"), "11:45 today");
        assert_eq!(expand_ratios("24:59"), "24:59");
    }

    #[test]
    fn ratio_excluded_for_chains_ids_and_letters() {
        assert_eq!(expand_ratios("1:30:00"), "1:30:00");
        assert_eq!(expand_ratios("A2:1"), "A2:1");
        assert_eq!(expand_ratios("2:1x"), "2:1x");
        assert_eq!(expand_ratios("1:01234"), "1:01234");
        assert_eq!(expand_ratios("the 2:1 ratio"), "the two to one ratio");
    }

    #[test]
    fn citation_volume_pages_prefix() {
        assert_eq!(expand_citation_volume_pages("Neurology. 2007;68:343-349."), "Neurology. 2007, volume sixty eight, pages 343-349.");
        assert_eq!(expand_citation_volume_pages("2011; 31:301"), "2011, volume thirty one, page 301");
        assert_eq!(expand_citation_volume_pages("2007;68(3):343-349"), "2007, volume sixty eight, issue three, pages 343-349");
        assert_eq!(expand_citation_volume_pages("2007 Jan;68:343"), "2007 Jan, volume sixty eight, page 343");
        assert_eq!(expand_citation_volume_pages("2007 Jan 5;68:343"), "2007 Jan 5, volume sixty eight, page 343");
        assert_eq!(expand_citation_volume_pages("2007;68:343\u{2010}349"), "2007, volume sixty eight, pages 343\u{2010}349");
    }

    #[test]
    fn citation_volume_pages_leaves_non_citations_alone() {
        // No year before the `;`: a list of scores, not a citation.
        assert_eq!(expand_citation_volume_pages("they won 3:1; 4:2 and 5:1"), "they won 3:1; 4:2 and 5:1");
        // Unrecognized page label or issue shape.
        assert_eq!(expand_citation_volume_pages("2007;68:e1234"), "2007;68:e1234");
        assert_eq!(expand_citation_volume_pages("2007;68 Suppl 1:S3"), "2007;68 Suppl 1:S3");
    }

    #[test]
    fn citation_volume_pages_end_to_end() {
        assert_eq!(
            normalize("Neurology. 2007;68:343\u{2010}349."),
            "Neurology. twenty oh seven, volume sixty eight, pages three forty three to three forty nine.",
        );
        assert_eq!(
            normalize("Cephalalgia. 2011;31(4):301-315."),
            "Cephalalgia. twenty eleven, volume thirty one, issue four, pages three oh one to three fifteen.",
        );
    }

    #[test]
    fn ratio_skips_journal_citation_volume_pages() {
        assert_eq!(expand_ratios("Neurology. 2007;68:343-349."), "Neurology. 2007;68:343-349.");
        assert_eq!(expand_ratios("Cephalalgia. 2011; 31:301-315."), "Cephalalgia. 2011; 31:301-315.");
    }

    #[test]
    fn numeric_range_accepts_unicode_hyphens() {
        assert_eq!(expand_numeric_ranges("343\u{2010}349"), "three forty three to three forty nine");
        assert_eq!(expand_numeric_ranges("343\u{2011}349"), "three forty three to three forty nine");
        // Same symmetric-spacing rule as the ASCII hyphen.
        assert_eq!(expand_numeric_ranges("10 \u{2010}2"), "10 \u{2010}2");
    }

    #[test]
    fn fraction_named_denominators() {
        assert_eq!(expand_fractions("1/2"), "one half");
        assert_eq!(expand_fractions("3/2"), "three halves");
        assert_eq!(expand_fractions("1/3"), "one third");
        assert_eq!(expand_fractions("2/3"), "two thirds");
        assert_eq!(expand_fractions("1/4"), "one fourth");
        assert_eq!(expand_fractions("3/4"), "three fourths");
        assert_eq!(expand_fractions("1/8"), "one eighth");
        assert_eq!(expand_fractions("5/8"), "five eighths");
        // Imperial-measurement range (sixteenths).
        assert_eq!(expand_fractions("5/16"), "five sixteenths");
        assert_eq!(expand_fractions("1/16"), "one sixteenth");
    }

    #[test]
    fn fraction_unnamed_denominator_falls_back_to_over() {
        // Beyond sixteenths, falls back to a plain cardinal "N over M" reading.
        assert_eq!(expand_fractions("5/37"), "five over thirty seven");
        assert_eq!(expand_fractions("1/17"), "one over seventeen");
    }

    #[test]
    fn fraction_in_context() {
        // Motivating real document: "1/3-octave band" — the fraction is
        // glued directly to a following hyphenated word, not free-standing.
        // Expanding "1/3" leaves the hyphen for Pass 10's existing
        // hyphen-to-space rule to handle, same as any other bare hyphen.
        assert_eq!(expand_fractions("1/3-octave band"), "one third-octave band");
        assert_eq!(normalize("a 1/3-octave band"), "A one third octave band");
        assert_eq!(normalize("Add 1/2 cup of sugar."), "Add one half cup of sugar.");
    }

    #[test]
    fn fraction_excluded_when_touching_a_letter() {
        // Not a fraction — an identifier/path-like token, or a
        // version/measurement shape with a letter glued directly onto the
        // denominator ("3/4G", "5/8ths"). split_alphanumeric (later in the
        // pipeline) handles the identifier case as its own token.
        assert_eq!(expand_fractions("a/b test"), "a/b test");
        assert_eq!(expand_fractions("F1/2 valve"), "F1/2 valve");
        assert_eq!(expand_fractions("3/4G network"), "3/4G network");
    }

    #[test]
    fn digit_hyphen_digit_preceded_by_letter_is_not_a_year_range() {
        // Regression: a contract-number-style ID ("F30602-76-C-003") was
        // being misread as a year range because the digit-hyphen-digit
        // scan didn't check what preceded the first digit run. Left to
        // Pass 10's split_alphanumeric instead, which spells the whole
        // alphanumeric token consistently.
        assert_eq!(
            expand_numeric_ranges("Contract F30602-76 was awarded"),
            "Contract F30602-76 was awarded",
        );
        assert_eq!(
            normalize("Contract F30602-76-C-003 was awarded."),
            "Contract F three zero six zero two seventy six C zero zero three was awarded.",
        );
        // A genuine year range is unaffected — nothing alphabetic precedes it.
        assert_eq!(expand_numeric_ranges("1976-77"), "nineteen seventy six to seventy seven");
    }

    #[test]
    fn year_range_with_five_plus_digit_half_spells_every_digit() {
        // Regression: `spell_bare_digit_group` only covers 1-4 digit runs;
        // the fallback used to silently leave a longer run bare
        // (`unwrap_or(first)`), and since this pass's match marks the whole
        // span non-raw, no later pass ever got a second chance at it —
        // those digits would go completely unspoken and untimeable by
        // forced alignment. Now falls back to spelling each digit
        // individually, the same fallback `split_alphanumeric` uses.
        assert_eq!(
            expand_numeric_ranges("30602-76"),
            "three zero six zero two to seventy six",
        );
    }

    #[test]
    fn hyphen_becomes_space() {
        assert_eq!(normalize("one-handed"), "One handed");
        assert_eq!(normalize("on-line"),    "On line");
    }

    #[test]
    fn hyphen_before_comma_dropped_without_space() {
        // A hyphen glued directly onto a comma (no space between) is an
        // elided-suffix marker ("16-, 32- or 64-bit"), not a word-joining
        // hyphen. Regression: turning it into a space like any other
        // hyphen left a stray space directly before the comma ("sixteen ,
        // thirty two"), the same TTS artifact Pass 0b exists to avoid —
        // Pass 0b runs too early in the pipeline to catch this one, since
        // the space doesn't exist yet until Pass 10 creates it.
        assert_eq!(
            normalize(
                "The example above is an 8-bit number, but it could just \
                 as easily have been written in 16-, 32- or 64-bit \
                 representation by using leading zeros"
            ),
            "The example above is an eight bit number, but it could just \
             as easily have been written in sixteen, thirty two or sixty \
             four bit representation by using leading zeros"
        );
        // An ordinary hyphenated word straddling a comma-adjacent context
        // is unaffected — only a bare hyphen directly glued to the comma
        // is dropped.
        assert_eq!(normalize("word-, other"), "Word, other");
        assert_eq!(normalize("64-bit"), "Sixty four bit");
    }

    #[test]
    fn hyphen_before_whitespace_dropped_without_doubling_space() {
        // "32- or" — a hyphen immediately followed by whitespace. The
        // source's own following space already separates the words, so
        // converting the hyphen to a second space as usual would double
        // it up ("thirty two  or"); it's dropped instead, same reasoning
        // as the comma case above.
        assert_eq!(normalize("32- or 64-bit"), "Thirty two or sixty four bit");
        // Ordinary mid-word hyphens (no adjacent whitespace) still convert
        // to a single space as before.
        assert_eq!(normalize("bit-mapped"), "Bit mapped");
    }

    #[test]
    fn full_citation_sentence() {
        let input = "as reported in <Ref-3> and <Ref-4>";
        assert_eq!(normalize(input), "As reported in Ref three and Ref four");
    }

    #[test]
    fn em_dash_becomes_comma() {
        assert_eq!(
            normalize("Statement 4b -- or, to conceptualize"),
            "Statement four b, or, to conceptualize"
        );
    }

    #[test]
    fn leading_em_dash_is_dropped_not_glued_comma() {
        // Regression: a block-quote attribution ("—Author, Year") has no
        // space after the dash, so converting it to a comma glued it
        // straight onto the first word — "—William Penn" -> ",William
        // Penn" — instead of a clean pause. There's nothing before it to
        // pause after anyway, so it should just disappear.
        assert_eq!(
            normalize("\u{2014}William Penn, 1694, in Britain Yearly Meeting"),
            "William Penn, sixteen ninety four, in Britain Yearly Meeting"
        );
    }

    #[test]
    fn mid_sentence_em_dash_unaffected_by_leading_dash_fix() {
        // The leading-dash special case must not change mid-sentence
        // behavior — the preceding space is consumed along with the dash,
        // so no space survives before the comma.
        assert_eq!(normalize("lines \u{2014} things"), "Lines, things");
    }

    #[test]
    fn em_dash_with_no_surrounding_spaces_still_becomes_comma_with_trailing_space() {
        // Rare/traditional typesetting with no spaces around the dash at
        // all (not the leading-attribution case, which is dropped
        // entirely) — still becomes a comma, with a trailing space added
        // so it doesn't glue onto the next word.
        assert_eq!(normalize("text\u{2014}more"), "Text, more");
    }

    #[test]
    fn em_dash_after_sentence_end_is_dropped_not_stacked_as_comma() {
        // Regression (seg62 of a real document): a citation attribution
        // ("—Jo Reed, 1972,...") glued directly after a sentence-ending
        // "." (or "…", the same document's ellipsis-before-attribution
        // convention) used to stack a comma right after the period —
        // "wedding.,Jo Reed" — gluing the comma to "Jo" *and* stacking
        // redundant punctuation. The period (or ellipsis) already ends the
        // sentence, so the dash is dropped entirely instead, with a single
        // space inserted since neither side of the glued dash had one.
        assert_eq!(
            normalize("they said they were planning a wedding.\u{2026} \u{2014}Jo Reed, 1972, in a letter"),
            "They said they were planning a wedding. Jo Reed, nineteen seventy two, in a letter"
        );
        assert_eq!(normalize("end.\u{2014}Bob said"), "End. Bob said");
        assert_eq!(normalize("really?\u{2014}she asked"), "Really? she asked");
    }

    #[test]
    fn em_dash_after_sentence_end_with_existing_spacing_collapses_cleanly() {
        // Same drop rule, but with normal spacing already present on both
        // sides — must not leave a double space behind.
        assert_eq!(normalize("end. \u{2014} Bob said"), "End. Bob said");
    }

    #[test]
    fn em_dash_after_comma_is_dropped_not_stacked_as_comma() {
        // Regression: a comma right before a glued em-dash used to still
        // turn the dash into its own comma, doubling up ("card,,
        // refused"). The comma already supplies the pause, so the dash is
        // dropped entirely instead, same as the sentence-terminal case
        // above.
        assert_eq!(
            normalize("refused my card,\u{2014}refused it peremptorily"),
            "Refused my card, refused it peremptorily"
        );
        assert_eq!(
            normalize("refused my card, \u{2014}refused it peremptorily"),
            "Refused my card, refused it peremptorily"
        );
    }

    #[test]
    fn em_dash_after_colon_is_dropped_not_stacked_as_comma() {
        // Regression: a bare colon isn't alphanumeric, so a dash glued
        // directly after one used to still fall into the generic "insert a
        // comma" branch — "problem:—" became "problem:," instead of just
        // dropping the redundant dash.
        assert_eq!(
            normalize("this our vastest social problem:\u{2014}"),
            "This our vastest social problem: "
        );
    }

    #[test]
    fn acronym_without_override_spells_out() {
        let overrides = HashMap::new();
        assert_eq!(process_token("SID", &overrides), "S I D");
    }

    #[test]
    fn journal_links_expanded() {
        assert_eq!(expand_journal_links("(AUGMENT,71279,)"), "Augment seven one two seven nine");
        assert_eq!(expand_journal_links("<OAD,2237,>"),      "O A D two two three seven");
        assert_eq!(expand_journal_links("(AUGMENT,14724,)"), "Augment one four seven two four");
        // lowercase name — not matched (DDD,xxx,bb style placeholders)
        assert_eq!(expand_journal_links("(DDD,xxx,bb)"),     "(DDD,xxx,bb)");
        // digits in second field required — not matched if non-digit
        assert_eq!(expand_journal_links("(DDD,xxx,)"),       "(DDD,xxx,)");
    }

    #[test]
    fn journal_link_tolerates_space_after_first_comma() {
        // Regression: a real citation in Engelbart's "Authorship Provisions
        // in AUGMENT" (1984) is typed as
        // "(AUGMENT, 81010,)" (space after the first comma), unlike every
        // other citation in that document. The match used to require the
        // digit run to start immediately after the comma, so this one
        // citation silently failed to match — leaving a bare 5-digit
        // number that, at the time, nothing downstream spelled out (Pass 8
        // only covered 1-4 digit runs; it has a 5+ digit fallback now, but
        // this pass still must run first — see the note above
        // `expand_journal_links_spanned`'s call site), so it reached the
        // TTS backend completely unspoken.
        assert_eq!(
            expand_journal_links("(AUGMENT, 81010,)"),
            "Augment eight one zero one zero",
        );
        assert_eq!(
            normalize("pp. 279-290. (AUGMENT, 81010,)"),
            "Pages two seventy nine to two ninety. Augment eight one zero one zero",
        );
    }

    #[test]
    fn brackets_stripped() {
        // The ")" right after "eight." is a plain deletion (inner neighbor
        // "." isn't alphanumeric, so it doesn't qualify as an aside
        // boundary). The "(" before "EEE,yy,cc)" sits between real prose
        // ("and") and an alphanumeric char ("E"), so it becomes a comma
        // instead; the trailing ")" is immediately followed by the
        // sentence's own "." (already a pause), so it stays a plain
        // deletion rather than doubling up.
        assert_eq!(normalize("<Ref-8>.) and (EEE,yy,cc)."), "Ref eight. and, E E E,yy,cc.");
    }

    #[test]
    fn bracket_glued_to_words_on_both_sides_becomes_a_space() {
        // Regression: a real document has "...for All Men(Philadelphia:
        // Book..." with no space before the "(" — plain deletion glued the
        // two words into "MenPhiladelphia", unreadable by TTS. Only fires
        // when *both* sides are glued (no space or punctuation already
        // there); a bracket with a space on either side still just deletes,
        // same as before.
        // "all" is single-word-overridden to lowercase "all" (avoids
        // letter-spelling "A L L") — mid-sentence that's fine, "All" -> "all".
        assert_eq!(
            normalize("A Vision for All Men(Philadelphia Book Committee"),
            "A Vision for all Men Philadelphia Book Committee",
        );
        // Also fires after a digit run expands into a word — the "(" ends
        // up glued to a word it wasn't glued to in the source text, once
        // "1782" becomes "seventeen eighty two".
        assert_eq!(
            normalize("the Year 1782(Chester Springs"),
            "The Year seventeen eighty two Chester Springs",
        );
        // Not glued on both sides, and neither side already has pause
        // punctuation — both brackets now become commas (aside-boundary
        // handling, see strip_brackets_spanned), not a plain deletion.
        assert_eq!(normalize("see (Augment) now"), "See, Augment, now");
    }

    #[test]
    fn unglued_parens_become_commas_as_an_aside_boundary() {
        // Regression: a real sentence, "Admiralty Arch (I had reached that
        // monument), or any other avenue..." — plain deletion left "Arch I
        // had" with no separator at all, which TTS read as "Arch the
        // First" (the bare "I" mistaken for a Roman-numeral/ordinal
        // suffix). The "(" (outer neighbor "Arch" is prose, no existing
        // pause) becomes a comma; the ")" is immediately followed by the
        // sentence's own "," already, so it stays a plain deletion rather
        // than doubling up.
        assert_eq!(
            normalize("Walk through the Admiralty Arch (I had reached that monument), or any other avenue."),
            "Walk through the Admiralty Arch, I had reached that monument, or any other avenue.",
        );
        // Symmetric case: neither side of the parenthetical already has
        // pause punctuation, so both "(" and ")" become commas.
        assert_eq!(
            normalize("the arch (built in 1912) was famous"),
            "The arch, built in nineteen twelve, was famous",
        );
        // A paren at the very start of the sentence has no outer neighbor
        // at all — no leading comma, plain deletion, same as before. Its
        // matching ")" still independently qualifies, though (its own
        // outer neighbor, "is", is ordinary prose with no existing pause).
        assert_eq!(normalize("(Note this) is important"), "Note this, is important");
    }

    #[test]
    fn all_caps_override_at_sentence_start_stays_capitalized() {
        // "all" -> "all" (built-in table) avoids "ALL" being spelled
        // letter-by-letter ("A L L"), but used verbatim it lowercases the
        // sentence's opening word — the final capitalize-sentence-start
        // pass fixes that up.
        assert_eq!(normalize("ALL rights reserved"), "All rights reserved");
    }

    #[test]
    fn colon_override_applied() {
        // The override key contains ':' not '.', which previously wasn't
        // matched by apply_punctuated_overrides.
        // Pass 2 strips the surrounding quotes (1 word ≤ 5).
        let n = Normalizer::builtin().with_overrides(parse_overrides("<4b:mi>\tfour B colon M I\n"));
        assert_eq!(n.normalize("\"<4b:mi>\""), "Four B colon M I");
    }

    #[test]
    fn override_forces_acronym_to_word() {
        // A 3-letter acronym that should be pronounced as a word, not spelled out.
        let mut overrides = HashMap::new();
        overrides.insert("sql".to_string(), "sequel".to_string());
        assert_eq!(process_token("SQL", &overrides), "sequel");
    }

    #[test]
    fn uppercase_punctuated_override_matches_case_sensitively() {
        // "No." (capital N) is the citation abbreviation ("Vol. 31, No. 7")
        // and should expand; lowercase "no." is the ordinary word "no"
        // ending a sentence and must be left alone — same distinction the
        // sentence splitter's ABBREVS list makes for the same word.
        let mut overrides = HashMap::new();
        overrides.insert("No.".to_string(), "Number".to_string());
        assert_eq!(apply_punctuated_overrides("See No. 7 for details.", &overrides), "See Number 7 for details.");
        assert_eq!(apply_punctuated_overrides("The answer is no.", &overrides), "The answer is no.");
    }

    #[test]
    fn number_abbreviation_requires_trailing_digit() {
        // A capitalized "No." that is the word "no" (its own sentence, or
        // starting one) must not expand; only "No." before a number does.
        let mut overrides = HashMap::new();
        overrides.insert("No.".to_string(), "Number".to_string());
        assert_eq!(
            apply_punctuated_overrides("No. These two frequencies produce identical signals.", &overrides),
            "No. These two frequencies produce identical signals.",
        );
        assert_eq!(apply_punctuated_overrides("Yes. No. Maybe.", &overrides), "Yes. No. Maybe.");
        assert_eq!(apply_punctuated_overrides("Vol. 31, No.7.", &overrides), "Vol. 31, Number 7.");
    }

    #[test]
    fn page_abbreviation_requires_trailing_digit() {
        // "p."/"pp." only mean "page(s)" when followed by a number (real
        // citation usage: "p. 234", "pp. 99-108"). A sentence ending in the
        // single-letter variable "p" ("...pressure p.") must not expand to
        // "page" just because it happens to spell the same key.
        let mut overrides = HashMap::new();
        overrides.insert("p.".to_string(), "page".to_string());
        overrides.insert("pp.".to_string(), "pages".to_string());
        assert_eq!(apply_punctuated_overrides("See p. 234 for details.", &overrides), "See page 234 for details.");
        assert_eq!(apply_punctuated_overrides("See pp. 99-108.", &overrides), "See pages 99-108.");
        assert_eq!(
            apply_punctuated_overrides("microphone observes sound pressure p.", &overrides),
            "microphone observes sound pressure p."
        );
    }

    #[test]
    fn inch_abbreviation_requires_preceding_number() {
        // "in." is only "inch" after a number; the word "in" ending a
        // sentence is left alone. Pluralized except after a bare "1", and
        // the sentence's period is kept when the abbreviation ends it.
        let mut overrides = HashMap::new();
        overrides.insert("in.".to_string(), "inch".to_string());
        let apply = |s| apply_punctuated_overrides(s, &overrides);
        assert_eq!(apply("keeps you signed in."), "keeps you signed in.");
        assert_eq!(apply("Log in. Then wait."), "Log in. Then wait.");
        assert_eq!(apply("a 3 in. pipe"), "a 3 inches pipe");
        assert_eq!(apply("a 1 in. pipe"), "a 1 inch pipe");
        assert_eq!(apply("a 1.5 in. pipe"), "a 1.5 inches pipe");
        assert_eq!(apply("cut it to 12in. and stop"), "cut it to 12 inches and stop");
        assert_eq!(apply("It is 12 in."), "It is 12 inches.");
        assert_eq!(apply("It is 1 in. Then trim it."), "It is 1 inch. Then trim it.");
    }

    #[test]
    fn number_units_expand_only_after_a_number() {
        let apply = |s| apply_punctuated_overrides(s, &HashMap::new());
        assert_eq!(apply("about 70 ms"), "about 70 milliseconds");
        assert_eq!(apply("about 70ms ago"), "about 70 milliseconds ago");
        assert_eq!(apply("a 1 ms pulse"), "a 1 millisecond pulse");
        assert_eq!(apply("a 1.5 ms pulse"), "a 1.5 milliseconds pulse");
        assert_eq!(apply("a 1,000 ms pulse"), "a 1,000 milliseconds pulse");
        assert_eq!(apply("5 µs and 5 μs"), "5 microseconds and 5 microseconds");
        assert_eq!(apply("3 ns, 4 KB, 5 MB, 6 GB"), "3 nanoseconds, 4 kilobytes, 5 megabytes, 6 gigabytes");
        // No number directly before: left alone, including the title "Ms".
        assert_eq!(apply("Ms Smith said ms"), "Ms Smith said ms");
        assert_eq!(apply("see 3. ms"), "see 3. ms");
        // A longer word that merely starts with the abbreviation.
        assert_eq!(apply("5 msg and 5 msec"), "5 msg and 5 msec");
        // dB: singular only for exactly 1; "dBA" is a different unit.
        assert_eq!(apply("a 1 dB step, 3 dB, 0.5 dB, 1dB"), "a 1 decibel step, 3 decibels, 0.5 decibels, 1 decibel");
        assert_eq!(apply("40 dBA"), "40 dBA");
        // Irregular plurals come from the table, not an added "s".
        assert_eq!(apply("1 ft tall, 6 ft deep"), "1 foot tall, 6 feet deep");
        assert_eq!(apply("a 1 lb bag, 5 lb, 10 lbs"), "a 1 pound bag, 5 pounds, 10 pounds");
        assert_eq!(apply("1 cm, 5 cm, 2 cm2"), "1 centimeter, 5 centimeters, 2 square centimeters");
        // End to end: a bare "dB" still gets the word override.
        assert_eq!(normalize("A 1 dB change in dB SPL."), "A one decibel change in decibels S P L.");
    }

    #[test]
    fn lowercase_punctuated_override_still_matches_case_insensitively() {
        // Keys with no uppercase letter keep the historical
        // case-insensitive behavior — unaffected by the new rule.
        let mut overrides = HashMap::new();
        overrides.insert("vol.".to_string(), "volume".to_string());
        assert_eq!(apply_punctuated_overrides("Vol. 31", &overrides), "volume 31");
    }

    #[test]
    fn override_forces_long_acronym_to_spell_out() {
        // A >3-letter acronym that would normally be lowercased, but should
        // be spelled out letter-by-letter instead.
        let mut overrides = HashMap::new();
        overrides.insert("nasa".to_string(), "N A S A".to_string());
        assert_eq!(process_token("NASA", &overrides), "N A S A");
    }

    #[test]
    fn state_abbrevs_expanded() {
        assert_eq!(expand_state_abbrevs("Denver, CO"), "Denver, Colorado");
        assert_eq!(expand_state_abbrevs("Austin, TX and Albany, NY"),
                   "Austin, Texas and Albany, New York");
    }

    #[test]
    fn rate_units_expanded() {
        assert_eq!(expand_rate_units("cycles/sec"), "cycles per second");
        assert_eq!(expand_rate_units("cycles/min"), "cycles per minute");
        assert_eq!(expand_rate_units("bands/decade"), "bands per decade");
        assert_eq!(expand_rate_units("samples / sec"), "samples per second");
        // Plural source spelling still maps to a singular unit.
        assert_eq!(expand_rate_units("events/hrs"), "events per hour");
    }

    #[test]
    fn rate_units_ordinary_word_pairs_left_unchanged() {
        // None of these land on a rate-unit table entry, so they're left
        // alone without needing their own exclusion list.
        for pair in ["and/or", "his/her", "document/file", "human/computer", "he/she", "printer/graphic"] {
            assert_eq!(expand_rate_units(pair), pair);
        }
    }

    #[test]
    fn rate_units_do_not_touch_doi_or_phonetic_notation() {
        // Digit immediately before the "/" — no alphabetic word there.
        assert_eq!(expand_rate_units("10.1016/j.ijpsycho.2005.10.025"),
                   "10.1016/j.ijpsycho.2005.10.025");
        // Nothing but whitespace before the first "/"; "s" isn't a rate unit.
        assert_eq!(expand_rate_units("the fricative /s/"), "the fricative /s/");
    }

    #[test]
    fn rate_units_unknown_unit_left_unchanged() {
        assert_eq!(expand_rate_units("miles/gallon"), "miles/gallon");
    }

    #[test]
    fn state_abbrevs_unknown_code_left_unchanged() {
        // "ZZ" isn't a postal code — leave the comma-led pattern as-is.
        assert_eq!(expand_state_abbrevs("Nowhere, ZZ"), "Nowhere, ZZ");
    }

    #[test]
    fn state_abbrevs_only_fire_after_comma() {
        // Not preceded by ", " — must not be treated as a state code.
        assert_eq!(expand_state_abbrevs("the CO task"), "the CO task");
        // Word boundary: a longer all-caps run starting with a valid code
        // must not be truncated/matched.
        assert_eq!(expand_state_abbrevs("score, COOL"), "score, COOL");
    }

    #[test]
    fn state_abbrevs_mixed_case_with_period() {
        // Informal abbreviation style: capitalized letter, lowercase
        // letter, trailing period — e.g. "Los Angeles, Ca." At the end of
        // the chunk (no continuation after), the period is genuinely
        // terminal and is left untouched.
        assert_eq!(expand_state_abbrevs("Los Angeles, Ca."), "Los Angeles, California.");
        // Followed by whitespace + uppercase — a real new sentence, not a
        // splitter miss — also left untouched.
        assert_eq!(expand_state_abbrevs("Denver, Co. Today it rained."),
                   "Denver, Colorado. Today it rained.");
    }

    #[test]
    fn state_abbrevs_mixed_case_period_dropped_mid_sentence() {
        // The splitter doesn't break a sentence at "Ca." followed by a
        // lowercase continuation (it's not in ABBREVS, but Unicode
        // sentence-break heuristics don't fire here either) — so this
        // whole string arrives as one sentence chunk, and the period is
        // this abbreviation's own, not a real sentence boundary. Dropping
        // it avoids planting a fake sentence break with a lowercase
        // continuation right after.
        assert_eq!(expand_state_abbrevs("San Francisco, Ca. is my home."),
                   "San Francisco, California is my home.");
        assert_eq!(expand_state_abbrevs("Denver, Co. today"), "Denver, Colorado today");
    }

    #[test]
    fn state_abbrevs_skipped_after_lowercase_word() {
        // A lowercase word before the comma isn't a city name — "SD" here
        // is standard deviation.
        assert_eq!(expand_state_abbrevs("identify the mean, SD, and any potential outliers"),
                   "identify the mean, SD, and any potential outliers");
        assert_eq!(expand_state_abbrevs("downtown, CO"), "downtown, CO");
        assert_eq!(expand_state_abbrevs("my home, Ca. is nice"), "my home, Ca. is nice");
        // Capitalized multi-word city still expands.
        assert_eq!(expand_state_abbrevs("Sioux Falls, SD"), "Sioux Falls, South Dakota");
    }

    #[test]
    fn state_abbrevs_mixed_case_unknown_code_left_unchanged() {
        assert_eq!(expand_state_abbrevs("Nowhere, Zz."), "Nowhere, Zz.");
    }

    #[test]
    fn state_abbrevs_mixed_case_word_boundary() {
        // A letter right after the period disqualifies the match.
        assert_eq!(expand_state_abbrevs("Denver, Co.lorful"), "Denver, Co.lorful");
    }

    #[test]
    fn tag_expansion_fallback_unchanged() {
        // Lowercase tag word — not a citation/figure/table marker, left as-is.
        assert_eq!(expand_tags("see <em> note"), "see <em> note");
        // Uppercase word but non-digit suffix — rejected by try_expand_tag,
        // tag is re-emitted literally rather than dropped.
        assert_eq!(expand_tags("see <Ref-x> note"), "see <Ref-x> note");
    }

    #[test]
    fn footnote_marker_glued_to_preceding_word() {
        // Regression (real document, "cooler" segments 2/3/4/10/12): bracket
        // stripping used to delete "["/"]" with nothing put in their place,
        // leaving the spelled-out digit glued straight onto the previous
        // word — "resolution.[1]" -> "resolution.one". Same reference-marker
        // treatment as <Ref-N>, just square-bracket syntax.
        assert_eq!(normalize("resolution.[1] This is"), "Resolution. Footnote one. This is");
        // No terminal punctuation before the marker, and lowercase prose
        // continues right after it — genuinely mid-sentence, so it reads as
        // a comma-set-off aside rather than a fake new sentence (see
        // `footnote_marker_mid_sentence_reads_as_an_aside`).
        assert_eq!(normalize("at Woodbrooke[3] and Friends"), "At Woodbrooke, footnote three, and Friends");
    }

    #[test]
    fn footnote_marker_with_preceding_space_still_normalizes_cleanly() {
        // Not glued — a space already precedes the bracket. Consumed as
        // part of the match; the replacement supplies its own single
        // leading space either way, so the result is identical.
        assert_eq!(normalize("resolution. [1] This is"), "Resolution. Footnote one. This is");
    }

    #[test]
    fn footnote_marker_with_number_list() {
        assert_eq!(expand_footnote_markers("seen.[2,3] Also"), "seen. Footnotes two and three. Also");
        assert_eq!(expand_footnote_markers("seen.[2, 3] Also"), "seen. Footnotes two and three. Also");
        assert_eq!(expand_footnote_markers("seen.[2, 3, 4] Also"), "seen. Footnotes two, three, and four. Also");
    }

    #[test]
    fn footnote_marker_with_range() {
        assert_eq!(expand_footnote_markers("seen.[4–8] Also"), "seen. Footnotes four to eight. Also");
        assert_eq!(expand_footnote_markers("seen.[4-8] Also"), "seen. Footnotes four to eight. Also");
        assert_eq!(expand_footnote_markers("seen.[1, 4–8] Also"), "seen. Footnotes one and four to eight. Also");
        assert_eq!(
            normalize("Sounds in this range [4–7], and screams [6], are aversive."),
            "Sounds in this range, footnotes four to seven, and screams, footnote six, are aversive.",
        );
        // A range never reads as a list number, even leading the text.
        assert_eq!(expand_footnote_markers("[4–8] Utley"), "Footnotes four to eight. Utley");
        // Not a range: a dangling dash, or a decimal.
        assert_eq!(expand_footnote_markers("see [4–] note"), "see [4–] note");
        assert_eq!(expand_footnote_markers("see [0.4–2] s"), "see [0.4–2] s");
    }

    #[test]
    fn marker_at_start_of_text_is_a_list_number() {
        // Regression: a numbered-bibliography line starting with its own
        // bracketed number fell through to the "anything else" footnote
        // case and came out with a stray leading period
        // (". Footnote forty nine. Utley ...").
        assert_eq!(
            normalize("[49] Utley 2012; Calloway 2012."),
            "Forty nine. Utley twenty twelve; Calloway twenty twelve.",
        );
        assert_eq!(normalize("  [49] Utley"), "Forty nine. Utley");
        assert_eq!(expand_footnote_markers("*3* Utley"), "Three. Utley");
        // Not a plausible list number — still a footnote, no leading ". ".
        assert_eq!(expand_footnote_markers("[2,3] Utley"), "Footnotes two and three. Utley");
    }

    #[test]
    fn marker_at_chunk_start_mid_text_is_still_a_footnote() {
        // A chunk boundary (from an earlier pass) right before the marker
        // doesn't make it the start of the text.
        let chunks = vec![
            Spanned { text: "at Woodbrooke".into(), src: 0..13 },
            Spanned { text: "[3] and Friends".into(), src: 13..28 },
        ];
        assert_eq!(
            flatten(&expand_footnote_markers_spanned(chunks)),
            "at Woodbrooke, footnote three, and Friends",
        );
    }

    #[test]
    fn marker_punctuation_across_chunk_edges_is_not_doubled() {
        // Regression: the terminal punctuation deciding a marker's reading
        // was only looked for inside the marker's own chunk, so a chunk
        // edge between them doubled the period ("resolution.. Footnote
        // one." / "another. Footnote four..").
        let before = vec![
            Spanned { text: "resolution.".into(), src: 0..11 },
            Spanned { text: "[1] This".into(), src: 11..19 },
        ];
        assert_eq!(
            flatten(&expand_footnote_markers_spanned(before)),
            "resolution. Footnote one. This",
        );
        let after = vec![
            Spanned { text: "and another[4]".into(), src: 0..14 },
            Spanned { text: ". Next".into(), src: 14..20 },
        ];
        assert_eq!(
            flatten(&expand_footnote_markers_spanned(after)),
            "and another, footnote four. Next",
        );
    }

    #[test]
    fn footnote_marker_fallback_unchanged() {
        // Not a footnote marker — no digit run right after "[", or
        // unterminated — left alone for the existing generic bracket
        // handling to deal with.
        assert_eq!(expand_footnote_markers("see [a] note"), "see [a] note");
        assert_eq!(expand_footnote_markers("see [1 note"), "see [1 note");
    }

    #[test]
    fn footnote_marker_mid_sentence_reads_as_an_aside() {
        // Regression: a marker with no terminal punctuation before *or*
        // immediately after it — genuinely mid-sentence, lowercase prose
        // continuing right after — used to force ". Footnote N." as if it
        // were a sentence boundary, fracturing one sentence into fragments
        // with a stray lowercase word right after the fake period
        // ("test. Footnote three. and another..."). Now reads as a
        // comma-set-off aside instead, keeping it one sentence.
        assert_eq!(
            normalize("this is a test [^3] and another [^4]."),
            "This is a test, footnote three, and another, footnote four.",
        );
    }

    #[test]
    fn footnote_marker_immediately_before_terminal_punctuation_does_not_double_it() {
        // Regression: when the marker is followed with no space by the
        // sentence's own terminal punctuation, the old design still
        // inserted its own trailing period — "another[^4]." became
        // "another. Footnote four.." (doubled). The source's own "." now
        // closes the sentence once; the marker just reads as an aside
        // before it.
        assert_eq!(
            expand_footnote_markers("another[4]."),
            "another, footnote four.",
        );
        assert_eq!(
            expand_footnote_markers("what now[2]!"),
            "what now, footnote two!",
        );
    }

    #[test]
    fn caret_footnote_reference_marker_expands_the_same_as_bracket_marker() {
        // Markdown footnote-reference syntax ("[^3]") is the same marker
        // convention with an extra sigil, not a different shape — should
        // read identically to "[3]". Real-world glued case: "God.[^3]".
        assert_eq!(normalize("this is a test.[^3]"), "This is a test. Footnote three.");
        assert_eq!(normalize("resolution.[^1] This is"), "Resolution. Footnote one. This is");
        assert_eq!(
            expand_footnote_markers("seen.[^2,3] Also"),
            "seen. Footnotes two and three. Also",
        );
        // "^" with no digit run right after still falls through unchanged,
        // same as any other non-marker bracket content.
        assert_eq!(expand_footnote_markers("see [^a] note"), "see [^a] note");
    }

    #[test]
    fn asterisk_footnote_marker_glued_to_preceding_word() {
        // Same treatment as bracket markers, asterisk-delimited style — the
        // real convention in Engelbart's "Augmenting Human Intellect"
        // (1962). Marker style shouldn't change what gets spoken.
        assert_eq!(normalize("resolution.*1* This is"), "Resolution. Footnote one. This is");
    }

    #[test]
    fn asterisk_footnote_marker_with_preceding_space_still_normalizes_cleanly() {
        assert_eq!(normalize("resolution. *1* This is"), "Resolution. Footnote one. This is");
    }

    #[test]
    fn asterisk_footnote_marker_with_number_list() {
        assert_eq!(expand_footnote_markers("seen.*2, 3* Also"), "seen. Footnotes two and three. Also");
    }

    #[test]
    fn asterisk_footnote_marker_fallback_unchanged() {
        // Not a footnote marker — no digit run right after "*", or
        // unterminated — left alone (e.g. real emphasis, or an unrelated
        // stray asterisk).
        assert_eq!(expand_footnote_markers("see *italic* note"), "see *italic* note");
        assert_eq!(expand_footnote_markers("see *1 note"), "see *1 note");
    }

    #[test]
    fn space_before_comma_stripped() {
        // Regression: a real citation ("North Carolina Press , 1948") has a
        // stray space before the comma. Left in, the TTS backend vocalizes
        // a strange nonsense syllable there — the space carries no meaning
        // worth preserving, so it's dropped rather than just tolerated.
        assert_eq!(strip_space_before_comma("Press , 1948"), "Press, 1948");
        assert_eq!(strip_space_before_comma("multiple   , spaces"), "multiple, spaces");
        // No space before the comma — unaffected.
        assert_eq!(strip_space_before_comma("word,other"), "word,other");
        assert_eq!(
            normalize("North Carolina Press , 1948"),
            "North Carolina Press, nineteen forty eight",
        );
    }

    #[test]
    fn space_before_period_stripped() {
        // Same OCR/authoring-error artifact as the comma case, just before
        // a period instead — left in, it produces the same TTS artifact.
        assert_eq!(strip_space_before_period("the end ."), "the end.");
        assert_eq!(strip_space_before_period("multiple   . spaces"), "multiple. spaces");
        // No space before the period — unaffected.
        assert_eq!(strip_space_before_period("word.other"), "word.other");
        assert_eq!(normalize("the report was finished ."), "The report was finished.");
    }

    #[test]
    fn space_before_period_does_not_break_spaced_ellipsis() {
        // A spaced-out ellipsis ("right now. . . . A concern") has a space
        // before every period after the first — stripping those collapses
        // it to a contiguous run before Pass 5b (ellipsis) ever sees it,
        // which Pass 5b's contiguous-run branch already handles the same
        // way the spaced-form branch used to.
        assert_eq!(
            normalize("right now. . . . A concern"),
            "Right now A concern"
        );
    }

    #[test]
    fn space_before_other_medial_punctuation_stripped() {
        // Same OCR/authoring-error artifact as comma/period, extended to
        // the rest of Pass 0b's SPACE_BEFORE_PUNCT_EARLY set.
        assert_eq!(strip_space_before_comma("wait ; then go"), "wait; then go");
        assert_eq!(strip_space_before_comma("really ! now"), "really! now");
        assert_eq!(strip_space_before_comma("really ? now"), "really? now");
        assert_eq!(strip_space_before_comma("word )other"), "word)other");
        assert_eq!(strip_space_before_comma("word ]other"), "word]other");
        // No space — unaffected.
        assert_eq!(strip_space_before_comma("wait; then go"), "wait; then go");
    }

    #[test]
    fn space_after_opening_bracket_stripped() {
        assert_eq!(strip_space_after_opening_bracket("( above"), "(above");
        assert_eq!(strip_space_after_opening_bracket("[ above"), "[above");
        // No space — unaffected.
        assert_eq!(strip_space_after_opening_bracket("(above"), "(above");
        // Combined with Pass 0b's ")" handling, both sides get glued to
        // "(above)" before Pass 11 ever sees it — which, now that both
        // sides are glued to real prose, applies its existing parenthetical-
        // aside rule and turns the brackets into commas.
        assert_eq!(
            normalize("in Mexico City ( above ), the weather"),
            "In Mexico City, above, the weather",
        );
    }

    #[test]
    fn quote_spacing_tightened() {
        // Space right after an opening quote.
        assert_eq!(tighten_quote_spacing("He said, \" Hi\""), "He said, \"Hi\"");
        // Space right before a closing quote.
        assert_eq!(tighten_quote_spacing("\"hello, \" he said."), "\"hello,\" he said.");
        // A lone trailing quote (no opener in this sentence — the multi-
        // sentence-quote case) with a space before it, right after a
        // period, is dropped entirely rather than just glued.
        assert_eq!(normalize("the end. \""), "The end.");
        assert_eq!(tighten_quote_spacing("the end. \""), "the end.");
        // Legitimate spacing (a real space before an opening quote) is left
        // alone.
        assert_eq!(tighten_quote_spacing("he said \"hi\""), "he said \"hi\"");
        // Quotes around punctuation alone keep their closing quote.
        assert_eq!(tighten_quote_spacing("A period (\".\") here"), "A period (\".\") here");
    }

    #[test]
    fn punctuated_key_matches_text_the_early_passes_rewrote() {
        // Pass 0c2 drops a closing quote after "!" and the space before a
        // closing quote; the keys go through the same passes, so they
        // still match.
        let overrides: HashMap<String, String> = [
            ("(this display)!!\"", "this display exclamation point exclamation point"),
            ("\"but wait, there's more . . . \".", "But wait, there's more!"),
            ("\".\"", "Baseline dot symbol"),
        ].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let n = Normalizer::from_map(overrides);
        assert_eq!(n.normalize("Showing (this display)!!\""), "Showing this display exclamation point exclamation point");
        assert_eq!(n.normalize("we would say, \"But wait, there's more . . . \"."), "We would say, But wait, there's more!");
        assert_eq!(n.normalize("A period (\".\") in the address"), "A period, Baseline dot symbol, in the address");
    }

    #[test]
    fn longer_punctuated_key_applies_before_one_it_contains() {
        let overrides: HashMap<String, String> = [("C++", "C plus plus"), ("+", "plus")]
            .into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        assert_eq!(Normalizer::from_map(overrides).normalize("Java (or C++), mostly"), "Java, or C plus plus, mostly");
    }

    #[test]
    fn trailing_quotes_after_terminal_punctuation_stripped() {
        // The real artifact this fixes: a nested closing-quote cluster right
        // after a sentence ends reaches TTS as literal punctuation.
        assert_eq!(
            strip_trailing_quotes_after_terminal_punctuation("high-minded.\"'"),
            "high-minded.",
        );
        assert_eq!(
            strip_trailing_quotes_after_terminal_punctuation("Really?'"),
            "Really?",
        );
        assert_eq!(
            strip_trailing_quotes_after_terminal_punctuation("No way!\""),
            "No way!",
        );
        // No quote after the terminal punctuation — unaffected.
        assert_eq!(
            strip_trailing_quotes_after_terminal_punctuation("the end."),
            "the end.",
        );
        // A quote elsewhere, not right after terminal punctuation — unaffected.
        assert_eq!(
            strip_trailing_quotes_after_terminal_punctuation("'quoted' text."),
            "'quoted' text.",
        );
        assert_eq!(
            normalize("He's a dear boy really and most high-minded.\"'"),
            "He's a dear boy really and most high minded.",
        );
    }

    #[test]
    fn curly_quotes_normalized_to_ascii() {
        assert_eq!(normalize_curly_quotes("\u{2018}hello\u{2019}"), "'hello'");
        assert_eq!(normalize_curly_quotes("\u{201C}world\u{201D}"), "\"world\"");
        // Runs before Pass 2 (short-quote stripping), which only recognizes
        // the ASCII double quote — a curly-quoted short phrase now gets
        // stripped the same as an ASCII-quoted one.
        assert_eq!(
            strip_short_quotes(&normalize_curly_quotes("the \u{201C}OK Key\u{201D} action")),
            "the OK Key action",
        );
    }

    #[test]
    fn bare_colon_replaced_with_comma_when_letter_before_and_space_after() {
        // Regression: a bare colon reaching the TTS backend as literal
        // punctuation produced a short garbled vocalization ("pew"/"puh"),
        // not a real pause. Dropping it outright (an earlier version of this
        // pass) proved awkward more often than useful across a real
        // document's colons — a comma in its place reads more naturally.
        assert_eq!(normalize_bare_colon("Note: something"), "Note, something");
        // Digit on either side (time, ratio) — no letter there, left alone.
        assert_eq!(normalize_bare_colon("The meeting is at 3:00 PM"), "The meeting is at 3:00 PM");
        assert_eq!(normalize_bare_colon("the ratio was 2:1"), "the ratio was 2:1");
        // No space and no digit after — not this pass's shape, left alone.
        assert_eq!(normalize_bare_colon("a:b"), "a:b");
        // The existing quoted "\":\"" -> "colon" override is a different,
        // narrower case (colon isn't preceded by a letter there) and stays
        // unaffected by this pass.
        assert_eq!(
            normalize("the symbol \":\" means separator"),
            "The symbol colon means separator",
        );
    }

    #[test]
    fn bare_colon_replaced_with_period_at_end_of_text() {
        // Regression: a sentence-final colon had nothing after it, so neither
        // the space nor the digit case matched and the literal colon reached
        // the TTS backend, producing an audible artifact.
        assert_eq!(normalize_bare_colon("the output buffer y:"), "the output buffer y.");
        assert_eq!(normalize_bare_colon("the output buffer y: "), "the output buffer y. ");
        assert_eq!(
            normalize("and accumulate them in the output buffer y:"),
            "And accumulate them in the output buffer y.",
        );
        // Mid-text colon still becomes a comma.
        assert_eq!(normalize_bare_colon("Note: something"), "Note, something");
        // Digit before — not this pass's shape, left alone.
        assert_eq!(normalize_bare_colon("ratio 2:"), "ratio 2:");
    }

    #[test]
    fn bare_colon_replaced_with_space_when_letter_before_and_digit_after() {
        // Regression: "arXiv:2301.02111." — the arXiv override only
        // rewrites the word token, leaving the colon to glue the
        // replacement word directly onto the spelled-out digits that
        // follow with no separation at all.
        assert_eq!(normalize_bare_colon("arXiv:2301"), "arXiv 2301");
        assert_eq!(
            normalize("arXiv:2301.02111."),
            "Archive twenty three oh one point zero two one one one.",
        );
    }

    #[test]
    fn on_the_hour_minutes_dropped_before_am_pm() {
        assert_eq!(drop_on_the_hour_minutes("9:00 AM"), "9 AM");
        // Two-digit hour.
        assert_eq!(drop_on_the_hour_minutes("11:00 PM"), "11 PM");
        // No space in the source — one is still inserted, not glued.
        assert_eq!(drop_on_the_hour_minutes("11:00PM"), "11 PM");
        // Case-insensitive, source case preserved.
        assert_eq!(drop_on_the_hour_minutes("9:00pm"), "9 pm");
        // Not on-the-hour — left alone.
        assert_eq!(drop_on_the_hour_minutes("9:15 AM"), "9:15 AM");
        // No AM/PM follows — a duration, not a clock time. Left alone.
        assert_eq!(drop_on_the_hour_minutes("5:00 remaining"), "5:00 remaining");
        // End to end: both halves of a time range spelled out correctly.
        assert_eq!(
            normalize("9:00 AM - 11:00 AM"),
            "Nine A M  eleven A M",
        );
    }

    #[test]
    fn am_pm_spelled_only_after_number() {
        assert_eq!(expand_am_pm_after_number("at 9 am."), "at 9 A M.");
        assert_eq!(expand_am_pm_after_number("by 5pm"), "by 5 P M");
        assert_eq!(expand_am_pm_after_number("10:30 PM"), "10:30 P M");
        // The plain word, a title, and longer words are left alone.
        assert_eq!(expand_am_pm_after_number("I am here"), "I am here");
        assert_eq!(expand_am_pm_after_number("PM Modi"), "PM Modi");
        assert_eq!(expand_am_pm_after_number("3 amps, 4 pmol"), "3 amps, 4 pmol");
        // End to end, including the lookback across Pass 0e's chunks.
        assert_eq!(normalize("I am here at 9 am."), "I am here at nine A M.");
        assert_eq!(normalize("Call 11:00PM tonight"), "Call eleven P M tonight");
        assert_eq!(normalize("by 5pm, then 6 pm."), "By five P M, then six P M.");
    }

    #[test]
    fn degree_minute_second_coordinates_expanded() {
        // The pattern this pass exists for: degrees + minutes.
        assert_eq!(
            expand_degree_minute_second("42° 15′"),
            "forty two degrees fifteen minutes",
        );
        // Seconds, on top of degrees + minutes.
        assert_eq!(
            expand_degree_minute_second("31° 15′ 20″"),
            "thirty one degrees fifteen minutes twenty seconds",
        );
        // A bare degree with no minutes group is left alone — that's the
        // generic "°" -> "degrees" override's job (built-in table), not
        // this pass's.
        assert_eq!(expand_degree_minute_second("72°F"), "72°F");
        // ...and that override inserts spaces on whichever side is glued
        // directly to a letter/digit, so "72degreesF" doesn't fuse together.
        assert_eq!(normalize("72°F outside"), "Seventy two degrees F outside");
        // A bare prime/double-prime with no preceding degree group is a
        // foot/inch mark, not arcminutes/arcseconds — left alone.
        assert_eq!(expand_degree_minute_second("5′ 6″ tall"), "5′ 6″ tall");
        // End to end: both halves of a coordinate pair, abbreviated
        // direction form.
        assert_eq!(
            normalize("42° 15′ N. lat. and 60° 35′ W. long."),
            "Forty two degrees fifteen minutes north latitude and sixty degrees thirty five minutes west longitude",
        );
        // End to end: already-spelled-out direction words need no override,
        // just the degree/minute expansion.
        assert_eq!(
            normalize("The frigate was then in 31° 15′ north latitude and 136° 42′ east longitude"),
            "The frigate was then in thirty one degrees fifteen minutes north latitude and one thirty six degrees forty two minutes east longitude",
        );
    }

    #[test]
    fn urls_replaced_with_link() {
        // Regression (real document, "cooler" segments 16/18): a URL's own
        // digits/dots/query-string punctuation used to get chewed up
        // piecemeal by later passes (dots read as "point", digit runs
        // spelled out letter by letter) — garbled, and there's no way to
        // tell a real word from something like "O2PmXP3Lx8paVS7D" without a
        // dictionary. Punting: the whole URL becomes "link".
        assert_eq!(
            normalize("found on the Pendle Hill YouTube channel at https://www.youtube.com/live/RzKVRChdN98?si=O2PmXP3Lx8paVS7D."),
            "Found on the Pendle Hill YouTube channel at link."
        );
        assert_eq!(
            normalize("accessed on October 30, 2023: https://www.lifeandpowerquakerdiscernmentonabuse.com/,"),
            "Accessed on October thirty, twenty twenty three: link,"
        );
        assert_eq!(
            normalize("video, accessed on October 30, 2023: https://quakerspeak.com/video/healing-from-abuse-in-quaker-communities/."),
            "Video, accessed on October thirty, twenty twenty three: link."
        );
    }

    #[test]
    fn url_without_trailing_punctuation() {
        assert_eq!(replace_urls("see http://example.com now"), "see link now");
    }

    #[test]
    fn bare_scheme_word_unaffected() {
        // "http"/"https" without "://" isn't a URL — left alone.
        assert_eq!(replace_urls("the http protocol"), "the http protocol");
    }

    #[test]
    fn year_range_not_expanded_when_followed_by_letter() {
        // Trailing letter right after the second digit run means this isn't
        // a bare year range (e.g. a version/identifier) — leave untouched.
        assert_eq!(expand_numeric_ranges("1976-77a"), "1976-77a");
    }

    #[test]
    fn parse_overrides_warns_on_missing_tab() {
        // No tab delimiter — line is ignored (warned, not inserted), rest
        // of the file still parses normally.
        let map = parse_overrides("noTabHere\nfoo\tbar\n");
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("foo"), Some(&"bar".to_string()));
    }

    #[test]
    fn parse_overrides_treats_bare_hash_line_as_comment() {
        // A lone "#" (no trailing space) is used as a visual separator
        // between comment blocks in an overrides file — previously fell
        // through to the tab-split logic and warned as a malformed entry.
        let map = parse_overrides("#\nfoo\tbar\n#\n");
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("foo"), Some(&"bar".to_string()));
    }

    #[test]
    fn digit_word_covers_all_digits_and_fallback() {
        assert_eq!(digit_word('0'), "zero");
        assert_eq!(digit_word('9'), "nine");
        assert_eq!(digit_word('x'), "");
    }

    #[test]
    fn bare_pipe_tokens_are_dropped() {
        // Byline divider, leading/trailing table-cell pipes, and a run of
        // pipes — each removed together with its separating whitespace.
        assert_eq!(normalize("Written by Carolyn | Published November"), "Written by Carolyn Published November");
        assert_eq!(normalize("| Role 1: | You are the clerk."), "Role one: You are the clerk.");
        assert_eq!(normalize("Role 1: |"), "Role one:");
        assert_eq!(normalize("Males | 2 | Females | 3 |||"), "Males two Females three");
    }

    #[test]
    fn pipe_inside_a_token_is_left_alone() {
        // Only a pipe with whitespace (or a text edge) on both sides is a
        // separator; absolute-value bars and glued pipes stay.
        // (The parens are stripped by Pass 11 as before; the bars stay.)
        assert_eq!(normalize("the integral of |H(f)| squared"), "The integral of |H f| squared");
        assert_eq!(normalize("a|b and c"), "A|b and c");
    }

    #[test]
    fn markdown_table_separator_row_pipes_do_not_count_as_words() {
        let blob = "| Age | Men | |---|---| | 0–4 | 0.3 |";
        assert!(!normalize(blob).contains('|'), "{}", normalize(blob));
        let without: usize = normalize("Age Men 0–4 0.3").split_whitespace().count();
        assert_eq!(spoken_word_count(blob), without);
    }

    #[test]
    fn dropping_a_pipe_keeps_source_ranges_for_neighbor_words() {
        let text = "Alpha | Beta";
        let result = normalize_with_spans(text);
        assert_eq!(result.text, "Alpha Beta");
        let beta = result.text.find("Beta").unwrap();
        let src = result.source_range(beta..beta + 4).unwrap();
        let chars: Vec<char> = text.chars().collect();
        let got: String = chars[src].iter().collect();
        assert_eq!(got, "Beta");
    }

    #[test]
    fn spoken_word_count_counts_normalized_words() {
        let text = "In 1976 it cost $45.50.";
        assert_eq!(text.split_whitespace().count(), 5);
        assert_eq!(spoken_word_count(text), normalize(text).split_whitespace().count());
        assert!(spoken_word_count(text) > 5);
    }

    // -----------------------------------------------------------------------
    // Span-mapping proof of concept
    // -----------------------------------------------------------------------

    #[test]
    fn spanned_year_range_matches_unspanned_text() {
        for input in ["1976-77", "in 1976-77 we", "pages 10-20", "no years here"] {
            let chunks = expand_numeric_ranges_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), expand_numeric_ranges(input));
        }
    }

    #[test]
    fn spanned_year_range_source_span_covers_original_digits() {
        let input = "in 1976-77 we";
        let chunks = expand_numeric_ranges_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "in nineteen seventy six to seventy seven we");

        // The expanded chunk should map back to "1976-77" in the original
        // input (chars 3..10).
        let offset_in_expansion = normalized.find("to").unwrap();
        assert_eq!(source_span_at(&chunks, offset_in_expansion), Some(3..10));

        // A char before the expansion falls in the leading "in " chunk,
        // which maps back to the whole unprocessed "in " run (0..3) — per
        // the chunk-granularity design, not a single character.
        assert_eq!(source_span_at(&chunks, 0), Some(0..3));
    }

    #[test]
    fn spanned_year_range_preserves_unexpanded_text_spans() {
        // No expansion happens — single passthrough chunk spanning the whole input.
        let input = "no years here";
        let chunks = expand_numeric_ranges_spanned(spanned_from_input(input));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].src, 0..input.chars().count());
    }

    #[test]
    fn spanned_hyphens_to_spaces_matches_unspanned_text() {
        for input in ["one-handed", "on-line", "no hyphens"] {
            let chunks = hyphens_to_spaces_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), input.replace('-', " "));
        }
    }

    #[test]
    fn spanned_hyphens_to_spaces_preserves_1to1_span() {
        let chunks = hyphens_to_spaces_spanned(spanned_from_input("on-line"));
        // Trivial 1:1 transform — chunk span is unchanged from the input.
        // This test exercises the pass in isolation, without the final
        // capitalize-sentence-start pass, so the leading "o" stays lowercase.
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].src, 0.."on-line".chars().count());
        assert_eq!(chunks[0].text, "on line");
    }

    #[test]
    fn spanned_passes_compose_across_pipeline() {
        // Chain two passes — confirms spans stay anchored to the *original*
        // input (not the intermediate output) when threaded through more
        // than one transform.
        let input = "the 1976-77 report-card";
        let chunks = spanned_from_input(input);
        let chunks = expand_numeric_ranges_spanned(chunks);
        let chunks = hyphens_to_spaces_spanned(chunks);

        let normalized = flatten(&chunks);
        assert_eq!(normalized, "the nineteen seventy six to seventy seven report card");

        // "to" still maps back to "1976-77" in the *original* input, even
        // though a later pass (hyphen removal) also ran on this text.
        let offset_in_expansion = normalized.find("to").unwrap();
        assert_eq!(source_span_at(&chunks, offset_in_expansion), Some(4..11));
    }

    #[test]
    fn spanned_strip_brackets_matches_unspanned() {
        // `<`/`>` still just delete (unaffected by the comma-boundary
        // logic, which only applies to `(`/`)`). The `)` right after
        // "8." stays a plain deletion too (inner neighbor "." isn't
        // alphanumeric). The `(` before "EEE,yy,cc)" sits between real
        // prose ("and") and an alphanumeric char ("E"), so it becomes a
        // comma; the trailing `)` is immediately followed by the
        // sentence's own "." (already a pause), so it stays a plain
        // deletion.
        let input = "<Ref-8>.) and (EEE,yy,cc).";
        let chunks = strip_brackets_spanned(spanned_from_input(input));
        assert_eq!(flatten(&chunks), "Ref-8. and, EEE,yy,cc.");
    }

    #[test]
    fn spanned_strip_brackets_preserves_spans_around_removed_chars() {
        // Bracket removal shrinks (or, for an aside-boundary paren, grows)
        // text — confirms surrounding text stays correctly anchored (not
        // silently made non-raw/unscannable) once a bracket char is
        // removed or replaced with a comma.
        let input = "see (Augment) now";
        let chunks = strip_brackets_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        // This test exercises the pass in isolation, without the final
        // capitalize-sentence-start pass, so the leading "s" stays lowercase.
        // Both brackets sit between real prose and an alphanumeric char on
        // either side, with no existing pause punctuation, so both become
        // commas rather than a plain deletion.
        assert_eq!(normalized, "see, Augment, now");
        let offset = normalized.find("now").unwrap();
        // The comma+space that replaced ")" is its own chunk, so "now"
        // starts its own chunk right after it, anchored to the original
        // "now" at source index 14..17.
        assert_eq!(source_span_at(&chunks, offset), Some(14..17));
    }

    #[test]
    fn spanned_ellipsis_matches_unspanned() {
        for input in ["foo...bar", "every ..., and", "no ellipsis here"] {
            let chunks = replace_ellipsis_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), replace_ellipsis(input));
        }
    }

    #[test]
    fn spanned_identifier_dots_matches_unspanned() {
        for input in ["4b.l", "Ref.dt", "end.", "..."] {
            let chunks = replace_identifier_dots_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), replace_identifier_dots(input));
        }
    }

    #[test]
    fn spanned_identifier_dots_source_span_is_single_char() {
        let input = "4b.l";
        let chunks = replace_identifier_dots_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "4b dot l");
        let offset_of_dot = normalized.find("dot").unwrap();
        // The "." was a single char at index 2 in the original input.
        assert_eq!(source_span_at(&chunks, offset_of_dot), Some(2..3));
    }

    #[test]
    fn spanned_comma_numbers_matches_unspanned() {
        for input in ["2,000 characters", "100,000 items", "(4b,12)", "no numbers"] {
            let chunks = expand_comma_numbers_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), expand_comma_numbers(input));
        }
    }

    #[test]
    fn spanned_comma_numbers_source_span_covers_digit_run() {
        let input = "I have 2,000 apples";
        let chunks = expand_comma_numbers_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "I have two thousand apples");
        let offset = normalized.find("two").unwrap();
        // "2,000" spans chars 7..12 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(7..12));
    }

    #[test]
    fn spanned_item_numbers_matches_unspanned() {
        for input in ["Item 71279", "Item 1000", "Item 42", "Items 71279"] {
            let chunks = spell_item_numbers_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), spell_item_numbers(input));
        }
    }

    #[test]
    fn currency_pounds_whole_amount() {
        // 4-digit runs read via the digit-group convention shared with Pass
        // 8/6c/4 (`spell_bare_digit_group`), same as a bare year would.
        assert_eq!(expand_currency("£2333"), "twenty three thirty three pounds");
    }

    #[test]
    fn currency_comma_grouped_whole_amount() {
        assert_eq!(expand_currency("$37,000"), "thirty seven thousand dollars");
        assert_eq!(
            expand_currency("$1,234.56"),
            "one thousand two hundred thirty four dollars and fifty six cents"
        );
    }

    #[test]
    fn currency_dollars_singular_for_one() {
        assert_eq!(expand_currency("$1"), "one dollar");
    }

    #[test]
    fn currency_pounds_singular_for_one() {
        assert_eq!(expand_currency("£1"), "one pound");
    }

    #[test]
    fn currency_decimal_amount() {
        assert_eq!(expand_currency("$45.50"), "forty five dollars and fifty cents");
        assert_eq!(expand_currency("$1.01"), "one dollar and one cent");
    }

    #[test]
    fn currency_decimal_zero_cents_omits_and_cents() {
        assert_eq!(expand_currency("$45.00"), "forty five dollars");
    }

    #[test]
    fn currency_decimal_not_two_digits_leaves_remainder_bare() {
        // Only exactly two fractional digits read as cents — anything else
        // is left for later passes, same "leave the rest for later"
        // fallback as Pass 4's numeric ranges.
        assert_eq!(expand_currency("$45.5"), "forty five dollars.5");
        assert_eq!(expand_currency("$45.500"), "forty five dollars.500");
    }

    #[test]
    fn currency_does_not_fire_mid_identifier() {
        // Same word-boundary reasoning as the cent-sign pass — a symbol
        // glued to a preceding letter isn't a bare amount.
        assert_eq!(expand_currency("v1$45"), "v1$45");
    }

    #[test]
    fn currency_symbol_with_no_digits_left_alone() {
        assert_eq!(expand_currency("cost is $ today"), "cost is $ today");
    }

    #[test]
    fn spanned_currency_matches_unspanned() {
        for input in ["£2333", "$45.50", "$45.00", "$1", "v1$45", "no currency here"] {
            let chunks = expand_currency_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), expand_currency(input));
        }
    }

    #[test]
    fn spanned_currency_source_span_covers_symbol_and_decimal() {
        let input = "price is $45.50 today";
        let chunks = expand_currency_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "price is forty five dollars and fifty cents today");
        let offset = normalized.find("forty five dollars and fifty cents").unwrap();
        // "$45.50" spans chars 9..15 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(9..15));
    }

    #[test]
    fn currency_through_full_pipeline() {
        assert_eq!(
            normalize("It costs £2333 or $45.50."),
            "It costs twenty three thirty three pounds or forty five dollars and fifty cents."
        );
    }

    #[test]
    fn cent_sign_singular_for_one() {
        assert_eq!(expand_cent_sign("1¢"), "one cent");
    }

    #[test]
    fn cent_sign_plural_for_other_amounts() {
        assert_eq!(expand_cent_sign("3¢"), "three cents");
        assert_eq!(expand_cent_sign("0¢"), "zero cents");
        assert_eq!(expand_cent_sign("100¢"), "one hundred cents");
    }

    #[test]
    fn cent_sign_bare_has_no_number_to_agree_with() {
        assert_eq!(expand_cent_sign("denoted ¢"), "denoted cents");
    }

    #[test]
    fn cent_sign_does_not_fire_mid_identifier() {
        // A digit run glued to a preceding letter isn't a bare amount —
        // same word-boundary reasoning as Pass 4's numeric-range check.
        assert_eq!(expand_cent_sign("v1¢"), "v1¢");
    }

    #[test]
    fn spanned_cent_sign_matches_unspanned() {
        for input in ["1¢", "3¢", "denoted ¢", "no cents here"] {
            let chunks = expand_cent_sign_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), expand_cent_sign(input));
        }
    }

    #[test]
    fn spanned_cent_sign_source_span_covers_digit_and_symbol() {
        let input = "cost is 1¢ today";
        let chunks = expand_cent_sign_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "cost is one cent today");
        let offset = normalized.find("one cent").unwrap();
        // "1¢" spans chars 8..10 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(8..10));
    }

    #[test]
    fn ordinal_suffix_expanded() {
        // Regression: "15th" used to reach split_alphanumeric, spelling only
        // the digit run ("fifteen") and leaving the "th" suffix stranded as
        // raw, unspoken letters — "fifteen th" instead of "fifteenth".
        assert_eq!(expand_ordinals("15th"), "fifteenth");
        assert_eq!(expand_ordinals("1st"), "first");
        assert_eq!(expand_ordinals("2nd"), "second");
        assert_eq!(expand_ordinals("3rd"), "third");
        assert_eq!(expand_ordinals("12th"), "twelfth");
        assert_eq!(expand_ordinals("22nd"), "twenty second");
        assert_eq!(expand_ordinals("20th"), "twentieth");
    }

    #[test]
    fn ordinal_suffix_case_insensitive() {
        assert_eq!(expand_ordinals("15TH"), "fifteenth");
        assert_eq!(expand_ordinals("1St"), "first");
    }

    #[test]
    fn ordinal_suffix_does_not_fire_mid_identifier() {
        // A letter directly before the digit run makes it part of an
        // identifier, not a number — same word-boundary reasoning as Pass
        // 4/6d. A letter right after the suffix means it's not a standalone
        // ordinal either.
        assert_eq!(expand_ordinals("F15th"), "F15th");
        assert_eq!(expand_ordinals("15thing"), "15thing");
    }

    #[test]
    fn ordinal_suffix_in_sentence() {
        assert_eq!(
            normalize("the 15th of March"),
            "The fifteenth of March"
        );
    }

    #[test]
    fn ordinal_suffix_glued_to_spoken_math() {
        // Spoken math keeps an ordinal suffix glued to its last word:
        // `\(n\)th` -> "nth", `\(n+1\)th` -> "n plus 1th".
        assert_eq!(normalize("the nth sample"), "The nth sample");
        assert_eq!(normalize("the n plus 1th sample"), "The n plus first sample");
    }

    #[test]
    fn spanned_ordinals_matches_unspanned() {
        for input in ["15th", "1st", "22nd", "F15th", "15thing", "no ordinal here"] {
            let chunks = expand_ordinals_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), expand_ordinals(input));
        }
    }

    #[test]
    fn spanned_ordinals_source_span_covers_digits_and_suffix() {
        let input = "on the 15th of March";
        let chunks = expand_ordinals_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "on the fifteenth of March");
        let offset = normalized.find("fifteenth").unwrap();
        // "15th" spans chars 7..11 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(7..11));
    }

    #[test]
    fn spanned_item_numbers_source_span_covers_whole_match() {
        let input = "see Item 71279 now";
        let chunks = spell_item_numbers_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "see Item seven one two seven nine now");
        let offset = normalized.find("seven one").unwrap();
        // "Item 71279" spans chars 4..14 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(4..14));
    }

    #[test]
    fn spanned_journal_links_matches_unspanned() {
        for input in ["(AUGMENT,71279,)", "<OAD,2237,>", "(DDD,xxx,bb)", "(DDD,xxx,)", "no links"] {
            let chunks = expand_journal_links_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), expand_journal_links(input));
        }
    }

    #[test]
    fn spanned_journal_links_source_span_covers_whole_link() {
        let input = "see (AUGMENT,71279,) now";
        let chunks = expand_journal_links_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "see Augment seven one two seven nine now");
        let offset = normalized.find("Augment").unwrap();
        // "(AUGMENT,71279,)" spans chars 4..20 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(4..20));
    }

    #[test]
    fn spanned_state_abbrevs_matches_unspanned() {
        for input in ["Denver, CO", "Austin, TX and Albany, NY", "Nowhere, ZZ", "the CO task", "score, COOL"] {
            let chunks = expand_state_abbrevs_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), expand_state_abbrevs(input));
        }
    }

    #[test]
    fn spanned_state_abbrevs_source_span_is_four_chars() {
        let input = "live in Denver, CO";
        let chunks = expand_state_abbrevs_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "live in Denver, Colorado");
        let offset = normalized.find("Colorado").unwrap();
        // ", CO" spans chars 14..18 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(14..18));
    }

    #[test]
    fn spanned_em_dash_matches_unspanned() {
        for input in ["Statement 4b -- or, to conceptualize", "lines \u{2014} things", "no dashes"] {
            let chunks = replace_em_dash_and_double_hyphen_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), replace_em_dash_and_double_hyphen(input));
        }
    }

    #[test]
    fn spanned_tokenize_matches_unspanned() {
        let overrides = HashMap::new();
        for input in ["one-handed", "UIS spoke", "AUGMENT's report", "0609 was the code"] {
            let chunks = tokenize_spanned(spanned_from_input(input), &overrides);
            assert_eq!(flatten(&chunks), tokenize(input, &overrides));
        }
    }

    #[test]
    fn spanned_tokenize_source_span_covers_whole_token() {
        let overrides = HashMap::new();
        let input = "see AUGMENT's report";
        let chunks = tokenize_spanned(spanned_from_input(input), &overrides);
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "see Augment's report");
        let offset = normalized.find("Augment's").unwrap();
        // "AUGMENT's" spans chars 4..13 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(4..13));
    }

    #[test]
    fn spanned_punctuated_overrides_matches_unspanned() {
        let mut overrides = HashMap::new();
        overrides.insert("e.g.".to_string(), "for example".to_string());
        for input in ["see e.g. this", "no override here"] {
            let chunks = apply_punctuated_overrides_spanned(spanned_from_input(input), &punctuated_keys(&overrides));
            assert_eq!(flatten(&chunks), apply_punctuated_overrides(input, &overrides));
        }
    }

    #[test]
    fn spanned_punctuated_overrides_source_span_covers_matched_key() {
        let mut overrides = HashMap::new();
        overrides.insert("e.g.".to_string(), "for example".to_string());
        let input = "see e.g. this";
        let chunks = apply_punctuated_overrides_spanned(spanned_from_input(input), &punctuated_keys(&overrides));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "see for example this");
        let offset = normalized.find("for example").unwrap();
        // "e.g." spans chars 4..8 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(4..8));
    }

    #[test]
    fn spanned_full_pipeline_matches_normalize() {
        // Run every converted pass in the same order as `normalize()` and
        // confirm the flattened result matches exactly — the real proof
        // that the chunk-threading approach holds up end-to-end, not just
        // pass-by-pass.
        let overrides = HashMap::new();
        for input in [
            "Item 71279 was reported in 1976-77, see (AUGMENT,71279,) near Denver, CO -- relevant.",
            "as reported in <Ref-3> and <Ref-4>",
            "one-handed on-line 0609",
            "no special handling needed here",
        ] {
            let chunks = spanned_from_input(input);
            let chunks = replace_urls_spanned(chunks);
            let chunks = apply_punctuated_overrides_spanned(chunks, &punctuated_keys(&overrides));
            let chunks = expand_tags_spanned(chunks);
            let chunks = expand_footnote_markers_spanned(chunks);
            let chunks = expand_numeric_ranges_spanned(chunks);
            let chunks = replace_em_dash_and_double_hyphen_spanned(chunks);
            let chunks = replace_number_sign_spanned(chunks);
            let chunks = replace_identifier_dots_spanned(chunks);
            let chunks = spell_item_numbers_spanned(chunks);
            let chunks = expand_journal_links_spanned(chunks);
            let chunks = expand_comma_numbers_spanned(chunks);
            let chunks = expand_state_abbrevs_spanned(chunks);
            let chunks = tokenize_spanned(chunks, &overrides);
            let chunks = strip_brackets_spanned(chunks);
            let chunks = replace_ellipsis_spanned(chunks);

            let expected_text = replace_urls(input);
            let expected_text = apply_punctuated_overrides(&expected_text, &overrides);
            let expected_text = strip_short_quotes(&expected_text);
            let expected_text = expand_tags(&expected_text);
            let expected_text = expand_footnote_markers(&expected_text);
            let expected_text = expand_numeric_ranges(&expected_text);
            let expected_text = replace_em_dash_and_double_hyphen(&expected_text);
            let expected_text = replace_number_sign(&expected_text);
            let expected_text = replace_identifier_dots(&expected_text);
            let expected_text = spell_item_numbers(&expected_text);
            let expected_text = expand_journal_links(&expected_text);
            let expected_text = expand_comma_numbers(&expected_text);
            let expected_text = expand_state_abbrevs(&expected_text);
            let expected_text = tokenize(&expected_text, &overrides);
            let expected_text: String = expected_text.chars()
                .filter(|c| !matches!(c, '(' | ')' | '<' | '>' | '[' | ']')).collect();
            let expected_text = replace_ellipsis(&expected_text);

            assert_eq!(flatten(&chunks), expected_text, "mismatch for input: {input:?}");
        }
    }

    #[test]
    fn spanned_full_pipeline_source_span_for_item_number() {
        // End-to-end span check: finding a selected passage's words — find
        // where an expanded word lands in the normalized text, then confirm
        // it maps back to the right span in the *original* sentence.
        let overrides = HashMap::new();
        let input = "see Item 71279 in the report";

        let chunks = spanned_from_input(input);
        let chunks = replace_urls_spanned(chunks);
        let chunks = apply_punctuated_overrides_spanned(chunks, &punctuated_keys(&overrides));
        let chunks = expand_tags_spanned(chunks);
        let chunks = expand_footnote_markers_spanned(chunks);
        let chunks = expand_numeric_ranges_spanned(chunks);
        let chunks = replace_em_dash_and_double_hyphen_spanned(chunks);
        let chunks = replace_number_sign_spanned(chunks);
        let chunks = replace_identifier_dots_spanned(chunks);
        let chunks = spell_item_numbers_spanned(chunks);
        let chunks = expand_journal_links_spanned(chunks);
        let chunks = expand_comma_numbers_spanned(chunks);
        let chunks = expand_state_abbrevs_spanned(chunks);
        let chunks = tokenize_spanned(chunks, &overrides);
        let chunks = strip_brackets_spanned(chunks);
        let chunks = replace_ellipsis_spanned(chunks);
        let chunks = capitalize_sentence_start_spanned(chunks);

        let normalized = flatten(&chunks);
        assert_eq!(normalized, "See Item seven one two seven nine in the report");

        let offset = normalized.find("seven one").unwrap();
        // "Item 71279" spans chars 4..14 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(4..14));
    }

    #[test]
    fn normalize_with_spans_source_range_for_footnote_marker() {
        // Aligning recorded audio: a forced-alignment transcript is
        // produced against *normalized* text ("Footnote one."), and needs
        // mapping back to a range in the *original* sentence text
        // ("...promising.*1*") to slice the right audio. Bracket and
        // asterisk markers must both round-trip correctly — asterisk is
        // new as of today's marker-reattachment/normalizer work, and had no
        // span-level coverage at all before this.
        for input in ["seem promising.*1*", "seem promising.[1]"] {
            let result = normalize_with_spans(input);
            assert_eq!(result.text, "Seem promising. Footnote one.");

            // "seem promising." is 15 chars; the 3-char marker ("*1*"/"[1]")
            // follows immediately at 15..18.
            let offset = result.text.find("Footnote one").unwrap();
            let match_len = "Footnote one.".len();
            let source = result.source_range(offset..offset + match_len);

            assert_eq!(source, Some(15..18), "input: {input:?}");
        }
    }

    #[test]
    fn normalize_with_spans_matches_normalize_text() {
        for input in [
            "Item 71279 was reported in 1976-77, see (AUGMENT,71279,) near Denver, CO -- relevant.",
            "\"<4b:mi>\"",
            "as reported in <Ref-3> and <Ref-4>",
        ] {
            assert_eq!(normalize_with_spans(input).text, normalize(input));
        }
    }

    #[test]
    fn normalize_with_spans_source_range_for_item_number() {
        let input = "see Item 71279 in the report";
        let result = normalize_with_spans(input);
        assert_eq!(result.text, "See Item seven one two seven nine in the report");

        let offset = result.text.find("seven one").unwrap();
        let match_len = "seven one two seven nine".len();
        let source = result.source_range(offset..offset + match_len);

        // "Item 71279" spans chars 4..14 in the original input.
        assert_eq!(source, Some(4..14));
        assert_eq!(&input[4..14], "Item 71279");
    }

    #[test]
    fn normalize_with_spans_source_range_out_of_bounds_is_none() {
        let result = normalize_with_spans("short");
        assert_eq!(result.source_range(0..1000), None);
        // Empty/inverted range is also rejected.
        assert_eq!(result.source_range(2..2), None);
    }

    #[test]
    fn quoted_short_all_caps_token_spells_only_the_letters() {
        // Quotes (and other non-alphabetic characters the tokenizer leaves
        // attached to the word) shouldn't get letter-spelled along with a
        // short all-caps acronym/initial.
        assert_eq!(normalize("the letter 'I'."), "The letter 'I'.");
        assert_eq!(normalize("the word 'CAT'."), "The word 'C A T'.");
        assert_eq!(normalize("'AB' was written"), "'A B' was written");
        // Ordinary contractions aren't all-caps, so this branch never fires
        // for them — confirm they're unaffected.
        assert_eq!(normalize("it's fine"), "It's fine");
        assert_eq!(normalize("don't go"), "Don't go");
    }
}
