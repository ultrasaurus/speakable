# Sentence splitting

How `speakable-splitter` decides where sentences end, step by step, and
why each step exists. The crate's [README](../crates/speakable-splitter/README.md)
covers its API, the marker shapes it recognizes, and the length cap.

A sentence is the unit that gets synthesized, timed and highlighted, so a
boundary in the wrong place rarely fails loudly. Instead, highlighting
drifts from the audio, a citation is read as a sentence of its own, or a
TTS request grows to hundreds of words. Every rule below exists because
real text broke the plain Unicode split in one of these ways.

## Steps

`split` runs these per paragraph, in order (`split_paragraph` in
`crates/speakable-splitter/src/lib.rs`):

1. Hide periods that don't end a sentence: abbreviations, fixed phrases
   and URLs.
2. Split each line at Unicode (UAX #29) sentence boundaries, then recover
   any characters the split dropped.
3. Reattach footnote and citation markers to the sentence they cite, in
   three passes.
4. Restore the hidden periods.
5. Merge pieces that UAX #29 tore apart but that belong together:
   outline labels, initials, journal names.
6. Trim each sentence, and cut any sentence that's too long to speak in
   one request.

The output is always exact, in-order slices of the input: no character is
added or lost, and merging is plain concatenation of the original pieces.

## Paragraphs

- Each non-blank line is its own paragraph. Blank lines are an optional,
  equivalent separator, not a requirement.
- The last sentence of each paragraph has `paragraph_end` set. Sentences
  never span paragraphs.

## Periods that don't end a sentence

The period, `?` or `!` is replaced with a placeholder before splitting
and restored afterward, so UAX #29 never sees it.

- **Abbreviations** (`ABBREVS`): titles (`Mr`, `Dr`, `Prof`), places
  (`St`, `Ave`), Latin (`e.g`, `i.e`, `et al`, `etc`), months, company
  suffixes (`Inc`, `Ltd`) and citation words (`No`, `Vol`, `Fig`, `p`,
  `pp`, `Ch`, `Sec`).
  - The period is hidden only when no letter or digit comes right before
    the abbreviation, so `p` doesn't fire inside `shop.`.
  - The list stays short on purpose. An abbreviation that also ends real
    sentences would stop those sentences from splitting.
- **Fixed phrases** (`ABBREV_PHRASES`): an abbreviation that also ends
  real sentences is protected only inside one phrase. `Ed. Note` stays one
  sentence, while `and Ed. Then` still splits. Splitting `Ed. Note` would
  also put the halves of an `ed. note` → "Editor's Note" override in
  different sentences.
- **URLs**: every `.`, `?` and `!` inside an `http://` or `https://` URL,
  up to the next whitespace, is hidden.
  - UAX #29 ends a sentence at a period before a capital letter even with
    no space, so a DOI like `…%3E3.0.CO;2-N` split at `3.0.`. A `?` before
    a query string (`watch?si=…`) split too.
  - Punctuation at the URL's end (`see https://x.org/a.`, `…/a).`) still
    ends the sentence.

## The Unicode split

- Each line is split with `unicode_sentences()` (UAX #29).
- UAX #29 sometimes returns slices that skip characters between them: a
  spaced ellipsis followed by a quote (`more . . . ". Next.`), or a
  trailing `…` after the last period. `recover_dropped_char_ranges` gives
  each gap to a neighboring slice, so nothing is lost, whatever
  punctuation triggers the gap.
- The split works on byte ranges into the line, so the marker passes below
  can move boundaries before any text is sliced out.

## Footnote and citation markers

A marker always joins the sentence it cites: never a sentence of its own,
and never the start of the next sentence, whatever its brackets or
spacing. Which bracket contents count as a marker is decided by
`is_marker_body`, which is public so that code grouping sentences (for
example into TTS requests) agrees with the splitter on what a marker is.
Its four shapes:

1. **Numbers and ranges, comma-separated:** `1`, `18, 19`, `4–8`,
   `1, 4–8` (en dash or hyphen).
2. **Markdown footnote references:** `^` and a label, as in `[^3]`.
3. **Statement references:** a word, a number and an empty field, as in
   `(AUGMENT,71279,)`. A space after the first comma is allowed.
4. **Citation fields** (`is_citation_field_list`): see "Citations" below.

A bracketed clause that starts the next sentence and fits none of these
(`[Redacted]`) is left alone.

Three passes reattach markers, because UAX #29 tears them in three
different ways:

- **`reattach_marker_openers`**: a marker glued to the end of a sentence
  with no space (`resolution.[1]`). UAX #29's rule SB8a cuts it in half,
  leaving the opening bracket with the sentence. The pass also handles a
  quote that UAX #29 sweeps into the earlier slice along with the bracket
  (`shift.("Title"...`).
- **`reattach_marker_prefixes`**: a marker attached to the start of the
  following sentence, either because it was spaced from the sentence it
  cites, or because it's an asterisk marker (`*1*`), which UAX #29 never
  separates from what follows.
- **`reattach_citation_locator_tails`**: the trailing locator of a long
  citation, left behind as its own slice (see "Citations").

A marker that is a whole sentence by itself (no sentence before it in its
paragraph) stays a sentence. `is_footnote_marker` lets a synthesizer that
skips sentences without letters still speak it: once normalized, `[1]`
reads "Footnote one."

## Citations

The splitter doesn't parse citations: it doesn't tell an author from a
title or a year. Its goal is narrow: keep an in-text or bracketed
citation in one piece with the sentence it belongs to. A short fragment
of punctuation and numbers (a bare page locator, a lone year) is worth
avoiding, because it times and synthesizes badly out of proportion to its
length.

### Citation fields (marker shape 4)

A marker body is a citation when:

- It has two or more fields, split on top-level `,` or `;`. A separator
  inside a quoted title isn't a field boundary.
- Each field contains only letters, digits, whitespace and
  `. - : & ' , ; "` (straight or curly quotes).
- At least one field contains a digit. That's what separates a citation
  from a parenthetical aside: `(see appendix)` has no digit.

It doesn't check what each field is (author, year, locator), so one rule
covers many styles:

- APA: `Smith, 2022, p. 15`; `Smith et al., 2020`; `Smith & Jones, 2021`;
  `Smith, 2020; Jones, 2023`; a quoted title in place of an author.
- IEEE/Vancouver with locators: `3, pp. 6-10`; `3, Ch. 2, pp. 7-10`.
- Works and page references: `Fox, Works, 8:42`; `Fox, Works, 7:332,
  335-6`; a quoted title followed by `; Works of Isaac Penington, vol. 3:
  pp.133-7`.

### Locator tails

A full citation can contain a period of its own, the title's, right before
a publisher in parentheses:

```text
[Thomas Kelly, A Testament of Devotion. (New York: Harper & Brothers, 1941), p. 5]
```

UAX #29 splits after `Devotion.`. `reattach_marker_openers` reattaches the
publisher parenthetical, which is a citation-field marker. That leaves
the locator `, p. 5]` as a slice of its own, starting with punctuation.
`reattach_citation_locator_tails` merges such a slice onto the nearest
real slice before it.

`is_citation_locator_tail` reads the slice right to left, since the
closing `]` is the one certain anchor. The whole trimmed slice must be:

- optional closing punctuation from the citation (`)`, `"`, `'`), then
- an optional `,` or `;`, then
- an optional page keyword (`p.`, `pp.`, `page`, `pages`), then
- one or two dash-joined digit runs (`5`, `340-341`, `24—28`), then
- the closing `]` and an optional period.

The rule is narrow on purpose. It recognizes the leftover locator, not
brackets in general. A broader rule, merging everything up to the `]`
that closes an unmatched `[`, would swallow bracketed text that really is
several sentences, such as an optional clause in a contract or a
proposed deletion in a bill.

### Journal names in references

A reference entry like `J. Acoust. Soc. Am. 108, 1170–1180 (2000).` tears
into pieces at each abbreviated word, and so does a spelled-out name
(`Brain. 1998;121:233–41.`). Listing journal abbreviations in `ABBREVS`
can't keep up (real reference lists use hundreds), and short ones like
`Am.` would stop real sentences from splitting.

Instead, `merge_journal_names` merges a run of short name-like pieces
onto the piece after them, but only when that piece opens with a
citation locator:

- A name-like piece has at most six words and ends in a period. Each word
  is capitalized, a lowercase abbreviation (`acoust.`), or a connector
  (`of`, `and`, `für`, ...).
- A locator opens with a digit and has a page range (`1170–1180`), a
  year in parentheses, a `;` (`2004;24:940–6.`), or a PubMed or DOI tag.
- Requiring the locator keeps prose safe: in "Done. 5 minutes later, we
  left." the piece after `Done.` opens with a digit but has no locator
  shape.
- The article title before the journal stays its own sentence, which
  gives a natural pause between title and journal.

## Labels and initials

UAX #29 ends a sentence at any period followed by a space and a capital,
so a label or an initial becomes a sentence of its own.

- **Outline labels** (`merge_outline_labels`): `I. Introduction`,
  `5.7. Summing sinusoids`, `U.S. Department`.
  - A label is one or more `.`-separated parts, each digits, uppercase, or
    a lowercase Roman numeral up to 100, with at most four letters in all
    (`is_outline_label`): `I.`, `ii.`, `8.`, `5.2.3.1.`, `1.E.1.`, `U.S.`.
  - A piece containing whitespace is never a label, so `the U.S.` inside
    prose is untouched. `e.g.` and `Ph.D.` don't qualify.
  - A label merges onto the piece after it, and merging repeats while the
    result is still a label, so a glued chain like `1.A. By` (torn into
    `1.`, `A. `, `By`) merges all the way to the prose.
- **Initials** (`merge_initials`): `E. B. White wrote this.` and
  `Engelbart, D. C.` both split mid-name. A sentence that ends in a bare
  initial merges onto whatever follows, through as many initials as
  there are.
  - This over-merges on purpose. It can't tell `Engelbart, D. C.` from
    the contrived `His grade was a D. C. Smith got an A.` A missed split
    costs one long stretch without a pause; a citation split mid-name gets
    a hard pause every time it appears, and citations are far more common.
- **Whitespace is kept as written.** Merging concatenates the untrimmed
  pieces, so a glued label (`1.A.`, as in the Project Gutenberg License)
  stays glued and a spaced one keeps its single space. Sentences are
  trimmed once, at the end.

## Long sentences

A reference entry's author list can run hundreds of names with no
sentence-ending punctuation. `cap_sentence_length` runs last and cuts any
sentence whose estimated spoken length is over `MAX_SENTENCE_WORDS` (300)
into `ceil(estimate / 250)` roughly equal pieces.

- **Estimated, not normalized:** the splitter doesn't depend on a
  normalizer. `estimated_word_weight` splits each whitespace token at
  hyphens and dashes, then counts a 2–3 letter all-caps part as one word
  per letter, a part with digits as one word per digit (plus one if it
  also has letters), and anything else as one word. A token with no
  letters or digits counts zero.
  On a sample of real documents this was within a few percent of the
  normalized count (mean error about 0, 5th and 95th percentiles about
  ±6%).
- **Cut points:** after a word ending in `,` `;` `:` `—` or `–`, nearest
  the ideal position within 15 words, else at the ideal word boundary.
  Never inside a word.
- Only the last piece of a cut sentence can carry `paragraph_end`.
- `MAX_SENTENCE_WORDS` is the largest TTS request, in spoken words, a
  caller is expected to send. Callers that group sentences into requests
  should use the same limit.

## Open issues

- **Multi-clause citations still tear.** When a citation's internal
  period is followed by prose rather than a locator, the locator-tail
  rule doesn't apply, and the citation splits into several sentences.
  Two shapes seen in real text:
  - Two sources in one bracket, the second quoting the first: a works
    reference ending `…1831, VI, 172.` followed by `Quoted by Lewis
    Benson, "Friends and the Truth" … p. 57]`. Both halves are citations,
    but the text after `172.` is real content.
  - A note and a citation in one bracket: a sentence of the author's own
    commentary, then `See Ventures in Discipleship … (Scottsdale,
    Pennsylvania: Herald Press, 1984), p.140.]`. These are two different
    things, an annotation and a citation, so even a citation parser
    shouldn't treat them as one.
- **No structured citation model.** Nothing extracts author, title,
  publisher, year or locator; the splitter only merges or leaves whole
  slices. Real citations don't follow one grammar: the same book cited two
  ways in one document (with and without the publisher parenthetical),
  articles with a quoted title and `42, no. 3 (Autumn 2005): 314`, and
  author lists whose `et al.` and `ed.` end in periods of their own.

## Acknowledgements

The examples here come from the texts the crate's tests quote (see its
README), plus citation fragments from two essays published by
inwardlight.org: "Friends Consultation on Doing the Truth" and "Gospel
Order".
