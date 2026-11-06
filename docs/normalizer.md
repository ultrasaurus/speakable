# Text normalization

How `speakable-normalizer` turns a sentence into the string a TTS engine
should read aloud, pass by pass. 

Goals:
* Create a string the TTS engine will pronounce correctly and naturally
  (and what a human would likely say when reading the text aloud)
* Create a string with no bare digit characters left for forced alignment to
  silently drop, since alignment models only have letters in their vocabulary.

The crate's [README](../crates/speakable-normalizer/README.md) covers its API
and override tables.

Normalization is applied per-sentence, after sentence splitting. Pass
numbers below match the `// Pass N:` comments in `normalize_with_spans`
(`crates/speakable-normalizer/src/lib.rs`) exactly — if they drift, fix
the code comments first, since they're the source of truth.

---

## Processing order

- **Pass 0** — Replace URLs with `link` (runs before everything else — a
  URL's own digits/dots/punctuation would otherwise get chewed up
  piecemeal by every later pass). This is an editorial choice, since
  URLs are difficult to listen to.
- **Pass 0b** — Strip whitespace directly before a comma, or before
  `!`/`?`/`;`/`)`/`]` (`"Press , 1948"` → `"Press, 1948"`, `"in Mexico City
  ( above ),"` → `"in Mexico City (above),"`) — a stray space there,
  typically an OCR or authoring error, makes the TTS backend vocalize a
  strange nonsense syllable. `.` is handled separately, later, as Pass 5c;
  `:` is deliberately excluded (see Pass 0d)
- **Pass 0b2** — Strip whitespace directly after an opening `(` or `[` —
  the mirror image of Pass 0b, so `"( above )"` ends up `"(above)"` once
  both passes have run
- **Pass 0c** — Normalize curly quotes (`‘’`/`“”`) to their ASCII
  equivalents (`'`/`"`) — must run before Pass 1/Pass 2, which only
  recognize the ASCII characters
- **Pass 0c2** — Tighten spacing around a double quote: drop a stray space
  right after an opening `"` (`'He said, " Hi"'` → `'He said, "Hi"'`) and
  right before a closing `"` (`'"hello, " he said'` → `'"hello," he said'`).
  A closing `"` that ends up directly after `.`/`!`/`?` this way is dropped
  outright, same as Pass 5d, unless the quotes hold only punctuation
  (`(".")`, naming the period itself). Scoped to `"` only — a single quote is also the contraction/possessive apostrophe, so there's no reliable way to tell
  a real opening/closing single-quote apart from one of those
- **Pass 0d** — Rewrite a bare colon when it's acting as sentence
  punctuation rather than a digit separator: letter before, whitespace
  after → replace with a comma (`"Note: something"` → `"Note, something"`;
  dropping it outright, an earlier version of this pass, was tried but
  proved awkward more often than useful). Letter before, end of text
  after → replace with a period (`"buffer y:"` → `"buffer y."`). Letter
  before, digit directly after with no whitespace → replace with a space
  (`"arXiv:2301"` → `"arXiv 2301"`) — otherwise a word-only override
  (like `arXiv` → `archive`) leaves the colon to glue the replacement word
  straight onto the digits that follow. Doesn't touch a colon between
  digits (times, ratios)
- **Pass 0e** — Drop on-the-hour minutes from a clock time immediately
  before AM/PM (`"9:00 AM"` → `"9 AM"`, `"11:00PM"` → `"11 PM"`) — spelling
  out "zero zero" reads awkwardly next to a plain hour number. Doesn't
  touch a non-clock digit-colon-digit shape (`"5:00 remaining"`, no AM/PM
  follows) or a time with real minutes (`"9:15 AM"`)
- **Pass 0f** — Expand a degrees-minutes(-seconds) coordinate group
  (`"42° 15′"` → `"forty two degrees fifteen minutes"`) — anchored on the
  arcminute/arcsecond prime characters (`′`/`″`), never the ASCII
  apostrophe/quote, which are also used for feet/inches. Must run before
  Pass 1's generic `°` → "degrees" override, or the anchor this pass needs
  is gone before it gets a look
- **Pass 1** — Punctuated overrides (runs first, before quote-stripping, so
  keys like `"."` and `"*D"` can match before their surrounding quotes are
  removed). Each key goes through Passes 0–0f too, so it matches the text
  those passes leave
- **Pass 1b** — Spell `am`/`pm` as `A M`/`P M` right after a number
- **Pass 2** — Strip short inline quotes (≤5 words)
- **Pass 3** — Expand `<Tag-N>` markers
- **Pass 3b** — Expand footnote/citation markers (`[1]`, `[2,3]`, `[^1]`,
  `*1*`) to spoken form (`"Footnote one."`, `"Footnotes two and three."`)
- **Pass 3c** — Expand Bible verse references (`"Mt. 5:30"` → `"Matthew
  chapter five verse thirty"`, `"Ps. 40:7-8"` → `"Psalm forty verse seven
  to eight"`) — book table lives in `bible_refs.rs`. Must run before Pass 4
  and Pass 8, for the same "specific before generic" reason as everywhere
  else in this file — otherwise the chapter/verse digits get claimed and
  mis-expanded independently
- **Pass 3d** — Expand a journal citation's volume/issue prefix
  (`2007;68(3):343-349` → `2007, volume sixty eight, issue three, pages
  343-349`); the page numbers are left for Pass 4/8
- **Pass 4** — Expand numeric ranges (e.g. `1976-77` → `nineteen seventy
  six to seventy seven`, `300–3.4` → `three hundred to three point four`) —
  handles ASCII hyphen, Unicode hyphen (U+2010/U+2011) or en-dash (U+2013)
  as the separator, decimal numbers on either side, and optional spaces
  around the separator (symmetric only for the hyphens — see below); spells both sides itself (shares the
  Pass 8 digit-group helper) rather than leaving them bare, since Pass 8
  can't reach digits inside an already-expanded chunk
- **Pass 4b** — Expand fractions (`1/2` → `one half`, `2/3` → `two thirds`,
  `5/37` → `five over thirty seven`) — runs before Pass 8 for the same
  reason as Pass 4
- **Pass 4c** — Expand ratios (`2:1` → `two to one`,
  `1,000,000,000,000:1` → `one trillion to one`) — an `H:MM` clock-time
  shape is left alone
- **Pass 5** — Replace em-dashes and double-hyphens (` -- ` → `, `). A
  leading em-dash at the very start of the sentence (a block-quote
  attribution like `—William Penn, 1694,`) is dropped entirely rather than
  turned into a comma
- **Pass 5b** — Replace ellipsis (`...` or `…`) with a single separating
  space where one is needed, nothing otherwise. Must run before Pass 10 —
  tokenizing would split a bare `...` into three separate 1-char chunks
  first, so a later pass could never match the 3-char pattern again
- **Pass 5c** — Strip whitespace directly before a period (`"the end ."` →
  `"the end."`) — same artifact and fix as Pass 0b's comma case, just for a
  period. Must run after Pass 5b, not alongside Pass 0b — see Pass 5c below
  for why
- **Pass 5d** — Drop any run of `'`/`"` immediately after terminal
  punctuation (`.`/`!`/`?`) — `"high-minded.\"'"` → `"high-minded."` — a
  leftover closing-quote cluster right after a sentence ends reaches TTS as
  literal punctuation. Quotes are never spoken and Pass 2 already discards
  them wholesale rather than preserving balance/nesting, so this extends
  the same policy to end-of-sentence quote runs. Must run after Pass 0c
  (curly quotes already normalized to ASCII)
- **Pass 6** — All about digits and alphanumeric sequences:
  - **Pass 6a** — Currency amounts: a leading `£` or `$` immediately
    followed by a digit run, with an optional `.` plus exactly two
    fractional digits (`£2333` → `twenty three thirty three pounds`,
    `$45.50` → `forty five dollars and fifty cents`) — runs before Pass 6c
    (identifier dots) so a decimal amount's `.` is still claimed as part of
    this pass's match, not yet turned into `" point "`
  - **Pass 6b** — Number sign: a bare `#` immediately followed by a digit
    → `number ` (`#26.44` → `number 26.44`); a hashtag or `C#` (no digit
    right after the `#`) is left alone. Runs before Pass 6c/d/7 — see
    Implementation
  - **Pass 6c** — Identifier dots and decimal points: `4b.l` → `4b dot l`,
    `26.44` → `26 point 44`. Runs before Pass 6d/7 — see Implementation
  - **Pass 6d** — Spell Item/reference numbers digit-by-digit
    (`Item 71279` → `Item seven one two seven nine`) — runs before Pass 8
    so Item/Ref numbers keep their ID-style digit-by-digit reading rather
    than being shadowed by the generic bare-number rule
  - **Pass 6e** — Expand a cent-sign amount (`1¢` → `one cent`, `3¢` →
    `three cents`; a bare `¢` with no leading digit run → `cents`) — runs
    before Pass 8 for the same reason as Pass 6d
  - **Pass 6f** — Expand an ordinal number: a digit run immediately
    followed by `st`/`nd`/`rd`/`th` (`15th` → `fifteenth`, `1st` → `first`,
    `22nd` → `twenty second`) — runs before Pass 8/Pass 10 for the same
    reason as Pass 6d/6e, so the suffix doesn't get left stranded as raw,
    unspoken letters once the digit run is spelled out on its own
- **Pass 7** — Expand journal links (`(AUGMENT,71279,)` → `Augment seven
  one two seven nine`) — runs before Pass 8 so a journal link's digit run
  is still one raw, unclaimed chunk when this pass looks for the pattern
- **Pass 8** — Expand bare numbers not already handled above: comma-grouped
  (`2,000` → `two thousand`), digit-group style for bare 1-4 digit runs
  (`560` → `five sixty`, `1976` → `nineteen seventy six`), or digit-by-digit
  for a bare 5+ digit run (`87574` → `eight seven five seven four`)
- **Pass 8b** — Expand a rate-unit slash (`cycles/sec` → `cycles per
  second`, `bands/decade` → `bands per decade`) — only when the word after
  the `/` is a known rate unit, so ordinary word/word pairs (`and/or`,
  `his/her`) are left untouched
- **Pass 9** — Expand US state postal abbreviations in `"City, ST"`
  position (`Denver, CO` → `Denver, Colorado`)
- **Pass 10** — Tokenize and process each token:
  - Override lookup (single-word)
  - Alphanumeric splitting
  - Roman numeral expansion (all-caps stems, 2+ letters)
  - All-caps handling (letter-spell ≤3 letters, title-case longer)
  - Leading-zero digit sequences (`0609` → `zero six zero nine`)
  - Remaining hyphens → spaces
- **Pass 11** — Strip bracket characters (`<>`, `[]`, `()`)
- **Pass 11b** — Drop bare `|` tokens (a run of `|` with whitespace or the
  text edge on both sides) together with the whitespace separating them
  from a neighbor: `"a | b"` → `"a b"`. These are table-cell separators from
  a table that didn't parse, or byline dividers; they aren't spoken, but
  each is a whitespace-delimited token, so it would count as a word in
  `spoken_word_count`. A `|`
  inside a token (`|H(f)|`, `a|b`) is left alone.
- **Pass 12** — Capitalize the sentence-opening character if it's lowercase,
  unconditionally — regardless of whether it's original source text or an
  earlier pass's output (e.g. a single-word override like `sql` → `sequel`
  used verbatim). This text is only ever heard via TTS, never shown to a
  reader, so there's no reason to preserve a deliberately-lowercase original
  spelling (e.g. `iPhone`) here; only pronunciation matters, and a lowercase
  word opening a sentence has been observed to cause TTS artifacts

---

## Rules

### Pass 0b — Space before comma

Any run of whitespace immediately before a comma is dropped, leaving the
comma glued to whatever precedes it.

| Input | Output |
|-------|--------|
| `Press , 1948` | `Press, 1948` |
| `multiple   , spaces` | `multiple, spaces` |
| `word,other` | `word,other` *(no space — unaffected)* |

Found via a real citation (`North Carolina Press , 1948`) — the stray space
made the TTS backend vocalize a strange nonsense syllable there. Runs
first, before everything else, so no later pass has to account for the
extra whitespace.

The same artifact happens before a period too (`"the end ."`), but that
case is handled by a separate, later pass — see Pass 5c — rather than
folded in here.

---

### Pass 0c — Curly quotes

Maps `‘`/`’` → `'` and `“`/`”` → `"`. One character for one character, so
it can map blindly over every chunk without the flush/subdivide dance most
other passes need.

Must run before Pass 1 (e.g. the `":"` symbol-name override) and Pass 2
(short-quote stripping) — both only recognize the ASCII `'`/`"` characters,
so a curly quote from the source would silently skip both otherwise.

---

### Pass 0d — Rewrite bare colon

A colon is rewritten when it's acting as sentence punctuation, in three
cases:

- Letter immediately before, end of the text after (only whitespace
  follows) → replaced with a period.
- Letter immediately before, whitespace immediately after → replaced with
  a comma.
- Letter immediately before, a digit immediately after with no
  whitespace → replaced with a single space.

| Input | Output |
|-------|--------|
| `Note: something` | `Note, something` |
| `the output buffer y:` | `The output buffer y.` *(end of text — period, so the sentence ends with a terminal cadence rather than a hanging comma)* |
| `arXiv:2301.02111.` | `Archive twenty three oh one point zero two one one one.` *(word-only override plus glued digit — see below)* |
| `The meeting is at 3:00 PM` | `The meeting is at 3:00 PM` *(digit before — a time, left alone)* |
| `the ratio was 2:1` | `the ratio was 2:1` *(digit before — a ratio, left alone)* |
| `a:b` | `a:b` *(no whitespace or digit after — neither case matches, left alone)* |
| `the symbol ":" means separator` | `the symbol colon means separator` *(quote before, not a letter — the separate `":"` override handles this, unaffected)* |

Found by ear: a bare colon reaching the TTS backend as literal punctuation
produced a short garbled vocalization ("pew"/"puh"), not a real pause.
Dropping it outright (an earlier version of this pass) was tried, but on
review turned out awkward more often than useful across a real document's
colons — a comma in its place reads more naturally.

The glued-digit case was found via a real `arXiv:<id>` citation: Pass 1's
`arXiv` → `archive` override only rewrites the word token, so a
still-untouched colon right after it glued the replacement word straight
onto the spelled-out digits that follow (`"Archive:twenty three..."`) with
no separation at all. Scoped narrowly to letter-then-colon-then-digit so
it doesn't touch a colon between digits (times, ratios) or any other
glued-punctuation shape.

---

### Pass 0e — On-the-hour clock times

`:00` is dropped from an hour immediately before `AM`/`PM`
(case-insensitive). A leading space before `AM`/`PM` is always exactly one
in the output, whether or not the source had one.

| Input | Output |
|-------|--------|
| `9:00 AM` | `9 AM` |
| `11:00 PM` | `11 PM` |
| `11:00PM` | `11 PM` *(no space in source — one is still inserted)* |
| `9:00pm` | `9 pm` *(case-insensitive match, source case preserved)* |
| `9:15 AM` | `9:15 AM` *(real minutes — left alone)* |
| `5:00 remaining` | `5:00 remaining` *(no AM/PM — a duration, not a clock time, left alone)* |

Otherwise `"9:00 AM"` reaches Pass 10 as three separate tokens (`9`, `00`,
the bare colon), and the leading-zero rule spells the minutes out digit by
digit — technically correct but reads awkwardly next to a plain hour
number (`"nine zero zero A M"`). Must run before Pass 10 for the same
reason: by then it's too late to recognize `"9:00"` as one clock-time
unit.

---

### Pass 0f — Degree/minute/second coordinates

A degrees-minutes(-seconds) group is expanded to words. Minutes are
required for this pass to fire at all; seconds are optional on top.

| Input | Output |
|-------|--------|
| `42° 15′` | `forty two degrees fifteen minutes` |
| `31° 15′ 20″` | `thirty one degrees fifteen minutes twenty seconds` |
| `72°F` | `72°F` *(no minutes group — left for the generic `°` override below)* |
| `5′ 6″ tall` | `5′ 6″ tall` *(no preceding degree group — a foot/inch mark, left alone)* |

Anchored specifically on the arcminute/arcsecond prime characters (`′`
U+2032, `″` U+2033), never the ASCII apostrophe/quote — those are also used
for feet/inches (`5′ 6″` tall), and there's no way to tell the two usages
apart from the character alone. Requiring a minutes group directly after a
degree group is what disambiguates: a bare `′` with nothing degree-shaped
before it is left completely untouched.

A bare `°` with no following minutes group (a temperature, a generic
angle/geometry use) is deliberately left raw here — that's the job of a
plain `°` → `degrees` entry in the override table (Pass 1), which fires on
any leftover `°` this pass didn't already consume. Must run before that
override: once it replaces `°` with `degrees`, the anchor this pass needs
to recognize a following `′` as arcminutes (rather than a foot-mark) is
gone.

Direction context, when present, needs no special handling by this pass —
`"north latitude"`/`"east longitude"` are already ordinary words. The
abbreviated form (`N. lat.`, `S. lat.`, `E. long.`, `W. long.`) is handled
by four more override entries rather than code, since it's a
fixed, small set of combinations.

A degree symbol glued directly to letters on both sides with no minutes
group (`"72°F"`) still reads correctly: Pass 1's override substitution
inserts a separating space on whichever side is glued directly to a
letter/digit, so `"72°F"` → `"72 degrees F"` rather than fusing into
`"72degreesF"`. This spacing fix is generic to every punctuated override,
not specific to `°`.

---

### Pass 1 — Overrides file (punctuated)

Overrides come from the override table the `Normalizer` was built with:
the crate's built-in table (`global_overrides.txt`, `BUILTIN_OVERRIDES`),
with any table the caller layers over it.

Format: two tab-separated columns. Using a tab (not spaces) as the
delimiter allows keys to contain spaces (e.g. multi-word punctuated
phrases like `I. INTRODUCTION`). Lines starting with `# ` (hash space)
are comments; lines starting with `#` followed by a non-space are valid
override keys (e.g. `#x`, `#s`).

Keys containing any non-alphanumeric character are applied as full-text
find-and-replace before tokenization. Single-word (alphanumeric-only) keys
are matched during tokenization (Pass 10).

A key is written the way the source text reads, and is run through Passes
0–0f before matching, so it matches the text those passes leave:
`(this display)!!"` matches although Pass 0c2 drops the closing quote after
`!`. A match may span chunks that an earlier pass split by removing a
character. Longer keys are applied first, so `C++` → "C plus plus" fires
before `+` → "plus" can take its pluses.

Match is **case-insensitive**, unless the key itself contains an uppercase
letter — that signals a case-**sensitive** match, needed when even the
trailing period doesn't disambiguate the abbreviation from an ordinary word.
`No.` (the "Vol. 31, No. 7" citation abbreviation) is the motivating case:
lowercase `no.` is just the word "no" ending a sentence, and used to get
mis-expanded to "Number" by the old case-insensitive match. Only the key's
case matters for this; the value/replacement is unaffected either way, and
is always used **exactly as written**. This mirrors the sentence splitter's
`ABBREVS` list (`speakable-splitter`), which makes the same `No`/`no`
distinction case-sensitively for the same reason.

A few keys collide with an ordinary word ending a sentence, so they only
match in context:

- `p.`/`pp.` match only when a number follows (`p. 234`), not the variable
  `p` ending a sentence.
- `No.` matches only when a number follows (`No. 7`, `No.7`). Its
  case-sensitive key already skips lowercase `no.`, but the word "No" as its
  own sentence (`No. These two…`) is capitalized too. The splitter's
  `ABBREVS` still hides that period unconditionally, so `No.` as a sentence
  is glued onto the next sentence; we accept that as harmless.
- `in.` matches only right after a number (`3 in.`, `12in.`), not the word
  "in" ending a sentence (`signed in.`). The value is pluralized (`inches`)
  unless the number is a bare `1`. When the abbreviation also ends the
  sentence (end of text, or whitespace then an uppercase letter), a period
  is appended, since the key consumed the sentence's own period.

| Input | Output |
|-------|--------|
| `keeps you signed in.` | `keeps you signed in.` |
| `a 3 in. pipe` | `a 3 inches pipe` |
| `It is 1 in. Then trim it.` | `It is 1 inch. Then trim it.` |

Some unit abbreviations get the same number-only treatment from a hardcoded
table (`NUMBER_UNITS`) rather than the override table. A word-only override
key would match every occurrence, so `ms` would also rewrite the title "Ms".
They are matched case-insensitively, only right after a number that ends in
a digit (decimals and comma groups included), glued or spaced, and must not
be followed by another letter or digit. Each entry names its singular and
plural word; the singular is used only when the number is a bare `1`, and
the plural can be irregular (`ft` → `foot`/`feet`). A unit that also has a
word override (`dB` → `decibels`) still gets that override when no number
comes before it.

| Input | Output |
|-------|--------|
| `about 70 ms`, `about 70ms` | `about 70 milliseconds` |
| `a 1 ms pulse` | `a 1 millisecond pulse` |
| `a 1.5 ms pulse` | `a 1.5 milliseconds pulse` |
| `1 ft tall, 6 ft deep` | `1 foot tall, 6 feet deep` |
| `a 1 dB step`, `in dB` | `a 1 decibel step`, `in decibels` |
| `Ms Smith said` | `Ms Smith said` |
| `5 msg`, `40 dBA` | `5 msg`, `40 dBA` |

When the matched key is glued directly onto a letter/digit on either side
(no space or other punctuation there), a separating space is inserted on
that side of the replacement — `"72°F"` (`°` → `degrees`) becomes
`"72 degrees F"`, not `"72degreesF"`. Found via the `°` override: a
replacement substituted verbatim between two glued alphanumerics fuses
into one unreadable run.

---

### Pass 1b — AM/PM after a number

`am`/`pm` (any case, a whole word) is spelled `A M`/`P M` when the nearest
non-whitespace character before it is a digit. Anywhere else it is left
alone, so the word "am" keeps its own pronunciation. A word override can't
do this, since it matches every occurrence. The lookback crosses chunks,
because Pass 0e leaves `11:00PM` as `11`, an inserted space, and `PM`.

| Input | Output |
|-------|--------|
| `I am here at 9 am.` | `I am here at 9 A M.` |
| `by 5pm` | `by 5 P M` |
| `10:30 PM` | `10:30 P M` |
| `PM Modi`, `3 amps` | `PM Modi`, `3 amps` |

`PM Modi` still reaches the listener as `P M Modi`, from Pass 10's spelling
of short all-caps words, not from this pass.

---

### Pass 2 — Strip short inline quotes

Quoted phrases of 5 words or fewer have their surrounding quotation marks
removed. This prevents TTS mangling of brief quoted phrases.

| Input | Output |
|-------|--------|
| `the "OK Key" action` | `the OK Key action` |
| `called "viewspecs."` | `called viewspecs.` |

Longer quoted passages are left intact.

---

### Pass 3 — Tag expansion

Citation, figure, and table markers are expanded to spoken form.

| Input | Output |
|-------|--------|
| `<Ref-3>` | `Ref three` |
| `<Fig-1>` | `Figure one` |
| `<Table-2>` | `Table two` |
| `<Foo-3>` | `Foo three` *(fallback: any `<CapWord-N>`)* |
| `<Ref-1976>` | `Ref nineteen seventy six` |
| `<Ref-12345>` | `Ref 12345` *(5+ digits — not yet handled, left bare)* |

Tags where the word part is lowercase (e.g. `<em>`, `<b>`) are left
untouched to avoid mangling HTML. The trailing number is spelled out via the
same digit-group convention as Pass 8/Pass 4 (`spell_bare_digit_group`) —
this pass must do its own spelling, the same reason Pass 4 (numeric ranges)
does: once the tag is expanded the chunk is no longer raw, so Pass 8's
bare-number scan can never reach those digits afterward (chunk-granularity
design, see Implementation below). Left bare, they'd be invisible to forced
alignment entirely, not just unspoken awkwardly.

---

### Pass 3b — Footnote/citation markers

A marker glued directly onto preceding text with no space
(`"resolution.[1]"`, `"Woodbrooke[3]"`) is the case that most needs
handling — bracket-stripping (Pass 11) would otherwise delete the `[`/`]`
with nothing put in their place, leaving the spelled-out digit glued
straight onto the previous word (`"resolution.one"`).

What surrounds the marker decides how it reads — four cases, plus the
start-of-text case:

| Context | Input | Output |
|---------|-------|--------|
| Start of text (only whitespace before it) — a list number, not a footnote | `[49] Utley 2012` | `Forty nine. Utley twenty twelve` |
| Already at a sentence boundary (preceded by `.`/`!`/`?`/`…`) | `resolution.[1]` | `resolution. Footnote one.` |
| Immediately followed by terminal punctuation or a `,`/`;`/`:`, no space | `another[4].`, `settings [1, 2], empirical` | `another, footnote four.`, `settings, footnotes one and two, empirical` |
| Followed by lowercase prose (mid-sentence) | `test [3] and another` | `test, footnote three, and another` |
| Anything else (uppercase following, end of text, etc.) | `Woodbrooke[3] and Friends` *(capital "W" precedes — no signal either way)* | `Woodbrooke. Footnote three. and Friends` *(falls back to the boundary style)* |

Comma-lists follow the same rule (`seen.[2,3]` → `seen. Footnotes two and
three.`; mid-sentence, `[2,3]` → `, footnotes two and three,`). At the
start of text a comma-list isn't a plausible list number, so it keeps the
footnote reading, without a leading period (`Footnotes two and three.`).
A range item (`[4–8]`, en dash or hyphen, as imported journal citations
are written) reads `footnotes four to eight`, and mixes with single
numbers (`[1, 4–8]` → `footnotes one and four to eight`).

A comma right after a marker used to fall through to the "anything else"
row: `settings [1, 2, 3], empirical` came out `settings. Footnotes one,
two, and three., empirical`. Found on a Nature article import, where
nearly every citation sits before a comma.

The mid-sentence and immediately-followed-by-punctuation cases were both
found from the same real report: `"this is a test [^3] and another
[^4]."` used to come out `"this is a test. Footnote three. and another.
Footnote four.."` — two problems at once. Forcing every marker into its
own `"Footnote N."` sentence fractured one real sentence into fragments
with a stray lowercase word right after a fake period; and when the
marker was immediately followed by the sentence's own terminal
punctuation, that punctuation was never accounted for, so `"Footnote
four."` picked up a second, doubled period. Now genuinely mid-sentence
markers read as a lowercase, comma-set-off aside instead of a fake
sentence boundary, and the source's own terminal punctuation is left to
close the sentence once rather than duplicated.

Three delimiter styles all read identically: `[N]`, `*N*` (asterisk, as in
Engelbart's AUGMENT journal), and `[^N]` (Markdown footnote-reference
syntax) — a `^` immediately after `[` is just skipped before parsing the
digit run, since it's the same marker convention with an extra sigil, not
a different shape. `[a]` (no digit run right after the opener) or an
unterminated `[1` fall through unchanged, left for Pass 11's generic
bracket handling.

The `[^N]` case matters beyond pronunciation: the sentence *splitter*
independently needed its own fix for the same syntax, since without it, a
glued `[^N]` tears the sentence in two at the period, which
reaches this pass as two separate sentence fragments instead of one.

---

### Pass 3c — Bible verse references

Matches `<book> <chapter>:<verse>[-<verse2>]` and expands it to spoken
form. `<book>` is either a book's full name or one of its abbreviations
(abbreviations always require a trailing period, to avoid colliding with
ordinary short words); the book table (all 66 books, including numbered
books like `1 Corinthians`/`2 Samuel`) lives in `bible_refs.rs` rather than
inline in `lib.rs`, since it's long. A numbered book's leading digit
is spoken as an ordinal (`1 Corinthians` → `First Corinthians`, `2 Samuel`
→ `Second Samuel`, `3 John` → `Third John`), matching how the reference is
read aloud rather than how it's written.

Every book says "chapter" except Psalms, which is spoken as `"Psalm
<chapter> verse <verse>"` with no "chapter" word.

| Input | Output |
|-------|--------|
| `Mt. 5:30` | `Matthew chapter five verse thirty` |
| `Matthew 5:30` | `Matthew chapter five verse thirty` |
| `Ps. 73:21` | `Psalm seventy three verse twenty one` |
| `Psalm 73:21` | `Psalm seventy three verse twenty one` |
| `Ps. 40:7-8` | `Psalm forty verse seven to eight` |
| `1 Cor. 13:4` | `First Corinthians chapter thirteen verse four` |
| `Mt 5:30` *(no period)* | `Mt 5:30` *(not recognized as an abbreviation, left alone)* |
| `The meeting is at 5:30` | `The meeting is at 5:30` *(no book name before the colon, left alone)* |

**Known limitations** (deliberately out of scope for now, left unchanged):
- Cross-chapter ranges (`John 3:16-4:2`) aren't recognized — only a
  same-chapter verse range (`chapter:verse-verse2`) is.
- A reference with no verse (`Genesis 1`) or verse only (`v. 5`) isn't
  recognized — the pattern requires the full `chapter:verse` shape.

Must run before Pass 4 (numeric ranges) and Pass 8 (bare numbers) — both
would otherwise independently claim and mis-expand the chapter/verse
digits before this pass ever sees the whole reference as one raw chunk to
match against, the same "specific before generic" reasoning as every other
pass in this file.

---

### Pass 3d — Journal citation volume/issue/pages

The `;<volume>[(<issue>)]:` part of a citation of the form
`<year>[ <month>[ <day>]];<volume>[(<issue>)]:<pages>` is spelled out as
words, so TTS isn't left with a run of bare numbers and punctuation. Volume
and issue are read as cardinals; the pages are left raw for Pass 4/8, and the
pass only looks ahead to say `pages` (a range follows) or `page`.

| Input | Output |
|-------|--------|
| `2007;68:343-349` | `2007, volume sixty eight, pages 343-349` |
| `2011; 31:301` | `2011, volume thirty one, page 301` |
| `2007;68(3):343-349` | `2007, volume sixty eight, issue three, pages 343-349` |
| `2007 Jan 5;68:343` | `2007 Jan 5, volume sixty eight, page 343` |
| `they won 3:1; 4:2` | *(left alone — no year before the `;`)* |
| `2007;68:e1234`, `2007;68 Suppl 1:S3` | *(left alone — unrecognized shape; the colon stays bare)* |

Anchored on a 4-digit year (1500–2099) so a `;`-separated list of scores or
ratios isn't rewritten. The anchor can't rule out a year followed directly by
a `;` and a ratio (`in 1998; 3:1 odds`), which is rewritten as a citation.
Must run before Pass 4, which would turn the page range into a non-raw chunk
the lookahead can't read, and before Pass 4c, which would otherwise read
`68:343` as a ratio.

---

### Pass 4 — Numeric ranges

A numeric range — two number-like tokens separated by an ASCII hyphen (`-`),
a Unicode hyphen (`‐`/`‑`, U+2010/U+2011) or an en-dash (`–`, U+2013) — is
expanded to "to", and **both** sides are
spelled out. Either side may be an integer or a decimal (`3.4` → `three
point four`). A letter directly preceding the first digit run disqualifies
the match (see word-boundary check below). Despite the name, this pass was
never actually specific to years — the original pattern was just "digit
run, hyphen, digit run, no letter before it", which already matched any
bare integer range. Confirmed via a real document's page-range citation,
`pp. 99-108` (`pp.` is a separate override entry) — it worked
correctly before this pass was ever generalized to decimals/en-dash/spacing.

Originally this pass required the separator to be glued directly onto both
digit runs with no space (`1976-77`, not `1976 - 77`). Found via a real
document's frequency-range citation, `"300–3.4 kHz"` (en-dash, no spaces) —
generalized to also handle a decimal on either side and en-dash as a
separator. Spaced-hyphen support (`4 - 5`) was added defensively alongside
it, on the assumption a page-range citation could plausibly be typed that
way too, without a specific real document driving it — that untested
assumption is exactly what let the same generalization admit a real
false-positive: an unconstrained "either side may be spaced independently"
rule also matched a spaced-out negative number (`"10 -2"`) as a bogus
range. See the symmetric-spacing rule below, added specifically to close
that gap.

| Input | Output |
|-------|--------|
| `1976-77` | `nineteen seventy six to seventy seven` |
| `pages 10-20` | `pages ten to twenty` |
| `4 - 5` | `four to five` |
| `300–3.4` | `three hundred to three point four` |
| `100-12,000 Hz` | `one hundred to twelve thousand Hz` |
| `F30602-76-C-003` | *(left alone — see below)* |

**Comma-grouped halves.** Either half may be a comma-grouped integer
(`12,000`), using Pass 8's grouping rule: a comma followed by exactly three
digits and no fourth. A comma-grouped half is spelled as a magnitude with
`number_to_words` (`twelve thousand`), not in the digit-group style. Before
this, the scan stopped at the comma, so `100-12,000` matched as `100-12`,
and Pass 8 later read the leftover `,000` as "zero zero zero". A
comma-grouped number with a decimal part (`1,234.5`) isn't handled yet.

**Spacing around the separator.** An en-dash is typographically unambiguous
as a range marker (English prose doesn't use it for negation), so either
side may be spaced independently: `300–3.4`, `300 –3.4`, `300– 3.4`, and
`300 – 3.4` all match. A hyphen (ASCII or Unicode) is treated as ambiguous
with a minus sign, so it only counts as a range separator when the spacing is
**symmetric** — both sides spaced (`4 - 5`) or neither (`1976-77`) — never
just one side:

| Input | Output |
|-------|--------|
| `10 -2` | `10 -2` *(one-sided space — could be "ten, negative two"; left alone)* |
| `10- 2` | `10- 2` *(other one-sided case; left alone)* |
| `300– 3.4` | `three hundred to three point four` *(en-dash — either side independently spaced is fine)* |

Unlike Pass 3's `<Ref-N>` tags, this pass spells its own digits rather than
leaving them bare for Pass 8 — see Implementation's "Pipeline ordering
matters" note for why: once this pass expands a range, the chunk is no
longer raw, so Pass 8 can never reach those digits afterward. Left bare,
they'd be invisible to forced alignment entirely (no letters in its
vocabulary), not just unspoken awkwardly — confirmed in practice: a word
lookup for a year range like `1973-76` found no match at all, since the
unspelled digits were silently dropped before alignment even ran, not
merely mismatched.

Two related bugs, both found via a real document's contract-number
citations (`Contract F30602-76-C-003`):

- **Word-boundary check.** This pass used to match *any*
  digit-hyphen-digit run, with no check on what came before it — so
  `F30602-76` (a contract ID, not a year range) matched on `30602-76`,
  silently mis-splitting an alphanumeric identifier. Now it only fires when
  no letter directly precedes the first digit run; a letter-prefixed run
  is left raw for Pass 10's `split_alphanumeric` to handle as one
  consistent token instead (`F30602-76-C-003` → `F three zero six zero two
  seventy six C zero zero three`).
- **5+ digit fallback.** `spell_bare_digit_group` (the digit-group
  convention: `560` → "five sixty", `1976` → "nineteen seventy six") only
  covers 1-4 digit runs. When a half of the range was longer than that
  (`30602`, in the misfiring case above), the fallback used to just leave
  it as raw, unspoken digits — and since this pass's match already marks
  the chunk non-raw, no later pass got a second chance to fix it. Now
  falls back to spelling each digit individually instead, same as
  `split_alphanumeric`'s own fallback (Pass 10).

Note: a bare negative number like `-2` (no digit token before the hyphen)
is not matched by this pass and remains for later handling; "four to five"
vs. "minus two" is context-dependent and out of scope for this pass. A
negative number immediately *after* another number (`10 -2`) is also left
alone — see the symmetric-spacing rule above.

**Chain length and leading-zero halves.** A range is exactly *two* number
segments — this pass scans the full maximal chain of `number(-number)*`
segments before deciding, not just the first pair, and only expands when
there are exactly two. Found via a real document's ISBN and Library of
Congress catalog card number sitting right next to each other: `"ISBN
0-87574-297-3. ... Catalog Card # 91-061844."`

| Input | Output |
|-------|--------|
| `0-87574-297-3` | *(left alone — 4 segments, an identifier chain, not a range)* |
| `91-061844` | *(left alone — leading zero on the second half signals an ID, not a magnitude)* |
| `91-108` | `ninety one to one oh eight` *(2 segments, no leading zero — a genuine range)* |

- **3+ segments.** The pass used to look at only one hyphen pair at a
  time, so `"0-87574-297-3"` had its *tail* pair silently misread as a
  range (`"...two hundred ninety seven to three"`) — the fact that three
  more segments preceded it was invisible to a pairwise scan. Now the
  whole chain is scanned first and, if it turns out to have 3+ segments,
  the entire chain is skipped untouched — not just the first segment —
  so no sub-pair within it (e.g. its last two segments) gets matched on a
  later iteration either.
- **Leading-zero half.** `"91-061844"` has exactly two segments, so the
  chain-length check alone didn't catch it — it was misread as `"ninety
  one to zero six one eight four four"`. A leading zero (length > 1) on
  either half signals an ID/serial number, not a magnitude — the same
  convention Pass 8 and Pass 10 already use for bare numbers elsewhere in
  this file (`061844` alone would already be spelled digit-by-digit, not
  as a magnitude) — so a range match is rejected whenever either half has
  one, falling through to that existing digit-by-digit handling instead.

---

### Pass 4b — Fractions

A fraction — a bare digit run, `/`, another bare digit run, with no spaces
around the slash — is expanded to words. The numerator is always a plain
cardinal (`5` → `five`, not the digit-group style Pass 8 uses for
standalone bare numbers — a fraction's numerator reads as a magnitude, not
an ID). The denominator uses a named word when it's one of halves through
sixteenths (covers common fractions plus the imperial-measurement range,
e.g. `5/16 inch`), singular for a numerator of 1 and plural otherwise;
anything else falls back to a plain `"N over M"` reading.

| Input | Output |
|-------|--------|
| `1/2` | `one half` |
| `3/2` | `three halves` |
| `2/3` | `two thirds` |
| `1/4` | `one fourth` |
| `5/16` | `five sixteenths` |
| `5/37` | `five over thirty seven` *(no named word past sixteenths — falls back)* |
| `a/b` | *(left alone — see word-boundary check below)* |

Found via a real document's acoustics citation, `"1/3-octave band"` — the
fraction sits directly against a hyphenated word, not free-standing.
Expanding just the `"1/3"` portion and leaving the trailing `-octave` alone
is deliberate: Pass 10's existing hyphen-to-space rule already handles
whatever hyphen sits after a normalized word, so there's no need for this
pass to special-case it — `"1/3-octave band"` → `"one third-octave band"`
→ (Pass 10) → `"one third octave band"`.

**Word-boundary check.** A digit run preceded by a letter (`F1/2`, an
identifier) or a denominator immediately followed by a letter (`3/4G`, a
version/network shape) disqualifies the match — same reasoning as Pass 4's
own word-boundary check for numeric ranges. `1/4`, in `"1/4 mile"`, is not
disqualified by anything at all: the point of "always read as a fraction"
(rather than trying to detect a possible American-style bare `M/D` date
like `"the meeting is 3/4"`) is that a bare month/day with no year is rare
in real prose — usually written out (`"March 4"`) or with a year
(`"3/4/2024"`, itself outside this pass's scope since it isn't two bare
digit runs either).

---

### Pass 4c — Ratios

Two numbers separated by a colon, with no spaces, are read as `N to M`. Each
side is an integer, a comma-grouped integer, or a decimal, and is spelled as
a magnitude with `number_to_words` (a decimal part reads digit-group style
after `point`), not the year-style digit-group reading Pass 8 gives a bare
number.

| Input | Output |
|-------|--------|
| `2:1` | `two to one` |
| `16:9` | `sixteen to nine` |
| `1,000,000,000,000:1` | `one trillion to one` |
| `1.5:1` | `one point five to one` |
| `3:15` | `3:15` *(clock-time shape — see below)* |
| `1:30:00` | `1:30:00` *(3+ segments — a duration, left alone)* |
| `A2:1`, `2:1x` | *(left alone — touching a letter, part of an identifier)* |
| `1:01234` | *(left alone — leading zero signals an ID)* |
| `Foo;68:343` | *(left alone — a `;` before the first number, spaced or not, is never a ratio; Pass 3d handles the dated-citation form)* |

A match is treated as a clock time, and skipped, when the left side is 1-2
digits up to 24, the right side is exactly two digits up to 59, and neither
side has a comma or decimal. A ratio that fits that shape (`3:15`, `1:10`)
is indistinguishable from a time and reads as one; every other
number:number colon is a ratio. Runs after Pass 0e, so on-the-hour times
before AM/PM are already rewritten, and after Pass 3c, so Bible references
are already claimed. Must run before Pass 8, which would otherwise spell each
side separately and leave the bare colon between them.

---

### Pass 5 — Em-dash / double-hyphen

` -- ` (with mandatory surrounding spaces) is replaced with `, ` directly —
that pattern is always already well-formed. A bare `—` gets more careful,
context-dependent handling, since real documents glue it onto neighboring
text in several different ways:

- A preceding space is always consumed as part of the match, so it doesn't
  survive stranded before the comma (`lines — things` → `lines, things`,
  not `lines , things` — a space-before-comma is its own TTS artifact).
- If the dash has no trailing space (the `—Author` attribution
  convention), a space is added after the comma so it doesn't glue onto
  the next word.
- A comma is only inserted when the (space-adjusted) preceding character
  is **alphanumeric**. Any non-alphanumeric preceding character —
  sentence-terminal punctuation (`.`, `!`, `?`, `…`), a comma, a colon, or
  anything else punctuation-shaped — already reads as a pause or boundary
  on its own, so the dash is **dropped entirely** instead, no comma
  inserted. Stacking a second pause-marker right after one that's already
  there is redundant and just as artifact-prone as the glued-comma case. A
  space is inserted only if neither side of the dash already has one, so
  the words on either side don't glue together.
- A `—` at the very start of the whole sentence being normalized (nothing
  precedes it at all) is likewise **dropped entirely** — see
  `drop_leading_em_dash` in Implementation.

| Input | Output |
|-------|--------|
| `Statement 3c -- and` | `Statement 3c, and` |
| `lines — things` | `lines, things` |
| `text—more` *(no surrounding spaces at all)* | `text, more` |
| `—William Penn, 1694,` *(start of sentence)* | `William Penn, 1694,` |
| `marriage.… —Bob Blood, 1972,` *(dash right after a sentence-ending `…`)* | `marriage. Bob Blood, 1972,` |
| `end.—Bob said` *(glued both sides, after `.`)* | `end. Bob said` |
| `card,—refused it` *(dash right after a comma)* | `card, refused it` |
| `problem:—` *(dash right after a colon — not alphanumeric)* | `problem: ` |

---

### Pass 5b — Ellipsis

Two earlier approaches didn't hold up, worth knowing before touching this
again:

- Replacing `...` with a newline (to give the TTS backend a pause boundary
  instead of it treating the sentence as unfinished and looping) fixed the
  looping but caused a different artifact — a "taunt" mispronunciation.
- Stripping the marker to nothing avoided both, but a plain strip has no
  notion of *why* the pause was there — sitting directly between two words
  with no source-text space, it collapsed them together
  (`"paused...then"` → `"pausedthen"`), a real regression only caught
  later.

The current pass recognizes three shapes — a contiguous run of 3+ periods
(`...`, `....`), the single `…` codepoint, or the same run spaced out with
one space between each period (`". . ."`, `". . . ."`) — real documents use
whichever typographic convention interchangeably, and all read the same
way. It consumes the marker plus a leading space if present, and decides
what to insert based on what follows:

- A closing quote right after the marker is kept as-is
  (`foo..."` → `foo"`) — a quote isn't a word needing separation.
- Any whitespace right after the marker is consumed too, so the pass
  decides for itself whether exactly one space belongs there, regardless of
  how much (or how little) was in the source.
- If what's left is alphanumeric — more prose follows — exactly one space
  is inserted, unless the marker opened the sentence (nothing precedes it
  to separate from).
- Otherwise (punctuation follows, or end of text) nothing is inserted; the
  marker attaches directly to what's next.

| Input | Output |
|-------|--------|
| `paused...then continued` | `paused then continued` |
| `paused... then continued` | `paused then continued` |
| `paused … then continued` | `paused then continued` |
| `every ..., and` | `every, and` |
| `foo..." bar` | `foo" bar` |
| `...and so it began.` | `and so it began.` |
| `right now. . . . A concern` | `right now A concern` |
| `the U. S. Government` | `the U. S. Government` *(only 2 dots — not an ellipsis, left alone)* |

Must run before Pass 10 (tokenization) — tokenizing splits every
non-alphanumeric character, including each `.` in a bare `...`, into its
own chunk. A pass that scans for the 3-character `...` pattern afterward
can never see it as one unit again; only the single-codepoint `…` form
would still match. This was a real bug: ASCII `...` silently passed through
to the TTS backend untouched for as long as this pass ran last, while `…`
was (correctly) always stripped.

The spaced form (`". . . ."`) was a second, separately-found gap in the
same pass — a real document uses it in place of `...`, and neither the
contiguous-run check nor the `…` check recognized it at all, so every one
of those periods reached the TTS backend as literal, unhandled punctuation
instead of a pause. Requires 3+ dots to fire, the same threshold as the
contiguous form, so it doesn't misfire on a real 2-dot pattern like
`"U. S."`.

---

### Pass 5c — Space before period

Any run of whitespace immediately before a period is dropped, leaving the
period glued to whatever precedes it — the same OCR/authoring-error
artifact Pass 0b fixes for a comma, just for a period.

| Input | Output |
|-------|--------|
| `the end .` | `the end.` |
| `multiple   . spaces` | `multiple. spaces` |
| `word.other` | `word.other` *(no space — unaffected)* |

Not folded into Pass 0b, even though the fix is identical: Pass 0b runs at
the very start of the pipeline, before Pass 5b (ellipsis) has a chance to
run. Stripping the space before *every* period that early would collapse a
spaced-out ellipsis (`". . . ."`) into a run of adjacent-but-separately-
chunked single periods — each too short on its own for Pass 5b's
contiguous-run scan to recognize (the same "pattern can span chunk
boundaries" trap documented for Pass 2 in Implementation below) — so the
dots would reach the TTS backend as literal, unmerged punctuation instead
of a pause. Running this pass after Pass 5b sidesteps the conflict
entirely: any spaced ellipsis has already been resolved into its final
form by then, so every period this pass still sees is a genuine, isolated
one, not part of an ellipsis.

---

### Pass 5d — Trailing quotes after terminal punctuation

Any run of `'`/`"` immediately after a terminal punctuation mark (`.`,
`!`, `?`) is dropped entirely, leaving the mark itself in place.

| Input | Output |
|-------|--------|
| `high-minded."'` | `high-minded.` |
| `Really?'` | `Really?` |
| `No way!"` | `No way!` |
| `the end.` | `the end.` *(no trailing quote — unaffected)* |
| `'quoted' text.` | `'quoted' text.` *(quote not right after terminal punctuation — unaffected)* |

Found via a real document's leaked-audio artifact where the source
passage is one long single-quoted block containing a double-quoted piece
of dialogue — `'...."He's a dear boy really and most high-minded."'` — so
the target sentence's closing `."'` is the double quote closing that
inner dialogue immediately followed by the single quote closing the
surrounding block, not two marks of the same kind stacked together. That
cluster reached TTS as literal punctuation and preceded a hallucinated
tail. Quotes are never spoken, and
Pass 2 already discards quote marks wholesale rather than trying to
preserve balance/nesting; this pass extends the same policy to a
leftover quote run at the end of a sentence instead of trying to keep
quotes paired. Must run after Pass 0c — curly quotes are already
normalized to ASCII `'`/`"` by then, so this pass only has to recognize
one quote form per side.

---

### Pass 6a — Currency amounts

A leading `£` or `$` immediately followed by a digit run is expanded to
the amount plus `pound(s)`/`dollar(s)`, singular only for exactly `1`. An
optional `.` plus exactly two fractional digits right after the whole-part
digit run reads as cents/pence: `and <spelled cents> cent(s)`, appended
after the unit word — unless the fractional part is `00`, in which case it
reads no differently from a whole amount (no `"and zero cents"`).

| Input | Output |
|-------|--------|
| `£2333` | `twenty three thirty three pounds` *(4-digit group convention, same as a bare year)* |
| `$1` | `one dollar` |
| `£1` | `one pound` |
| `$45.50` | `forty five dollars and fifty cents` |
| `$1.01` | `one dollar and one cent` |
| `$45.00` | `forty five dollars` *(zero cents — omitted)* |
| `$45.5` | `forty five dollars.5` *(not exactly two fractional digits — only the whole part is claimed, remainder left bare)* |
| `v1$45` | `v1$45` *(symbol glued to a preceding letter — an identifier, not an amount; left alone, same word-boundary reasoning as Pass 4/6e)* |
| `cost is $ today` | `cost is $ today` *(no digit run after the symbol — nothing to expand)* |

Must run before Pass 6c (identifier dots) — otherwise a decimal amount's
`.` would already be turned into `" point "` before this pass ever saw
`$45.50` as one whole, still-raw shape to match against (the same "can't
reach across an already-expanded chunk" trap documented for Pass 4). Whole
and fractional digit runs are spelled out via the shared digit-group and
plain-cardinal conventions respectively (`spell_bare_digit_group` for the
whole part, same as Pass 4/6d/7/8; a plain cardinal for the 0-99
fractional part, same as Pass 4b's fraction numerator), not left bare for
Pass 8 — there is no later chance to reach digits inside an
already-expanded chunk, same reasoning as Pass 4's numeric ranges.

---

### Pass 6b — Number sign

A bare `#` immediately followed by a digit is replaced with `number `. Not
followed by a digit (a hashtag, or `C#`) — left untouched.

Must run before Pass 6c/d/7 spell the digits out, for the same reason as
Pass 6c below — a `#` glued to an already-expanded digit chunk would be
stranded with no adjacent raw text left to match against.

| Input | Output |
|-------|--------|
| `#26` | `number 26` |
| `#26.44` | `number 26.44` *(→ `number twenty six point forty four` once Pass 6c/7 run)* |
| `#quaker` | `#quaker` *(no digit after "#" — a hashtag, untouched)* |
| `C#` | `C#` *(no digit after "#" either)* |

---

### Pass 6c — Identifier dots and decimal points

A `.` followed by an alphanumeric character, not preceded by another `.`,
is replaced with ` dot ` or ` point `. Catches link notation like `4b.l`
and `.dt`, and decimal-style numbers like `26.44` or `3.14`.

Runs before Pass 6d/7 (which spell out the digit runs on either side) — see
[Pipeline ordering matters](#pipeline-ordering-matters-more-than-it-looks)
below — so it decides `point` vs. `dot` by looking directly at the raw
characters flanking the `.`: both sides digits → `point`; anything else
(including one side digit, one side letter) → `dot`.

| Input | Output |
|-------|--------|
| `4b.l` | `4b dot l` |
| `Ref.dt` | `Ref dot dt` |
| `.l` | `dot l` |
| `26.44` | `26 point 44` |
| `3.14` | `3 point 14` |
| `Ref.1974` | `Ref dot 1974` *(digits on only one side — still `dot`)* |

The same pass reads an `@` followed by an alphanumeric character as `at`,
covering email addresses and social handles. A leading space is added only
when the `@` is glued to a preceding letter/digit. A lone `@` (nothing
alphanumeric after) is left alone.

| Input | Output |
|-------|--------|
| `support@notem.us` | `support at notem dot us` |
| `follow @notemus` | `follow at notemus` |
| `a lone @ sign` | `a lone @ sign` |

---

### Pass 6d — Item/reference numbers

Multi-digit numbers (4+ digits) following `Item` or `Ref` are spelled out
digit-by-digit so TTS doesn't garble large IDs — these read as identifiers,
not magnitudes.

| Input | Output |
|-------|--------|
| `Item 71279` | `Item seven one two seven nine` |
| `Ref 14724` | `Ref one four seven two four` |
| `Item 1000` | `Item one zero zero zero` |
| `Item 42` | `Item 42` *(fewer than 4 digits — left to Pass 8)* |

Numbers with commas (e.g. `Item 2,345`) are excluded — Pass 6d only counts
contiguous digit runs, so a comma-grouped number never reaches the 4-digit
threshold here and falls through to Pass 8 instead.

---

### Pass 6e — Cent sign

A digit run immediately followed by `¢` (no space) is spelled out via the
same digit-group convention Pass 4/6d/7/8 share
(`spell_bare_digit_group`), with `cent`/`cents` agreement matching the
amount — singular only for exactly `1`, same reasoning as Pass 4b's
fraction numerator. A bare `¢` with no leading digit run has no number to
agree with and expands to plain `cents`.

| Input | Output |
|-------|--------|
| `1¢` | `one cent` |
| `3¢` | `three cents` |
| `100¢` | `one hundred cents` |
| `denoted ¢` | `denoted cents` |
| `v1¢` | `v1¢` *(digit run glued to a preceding letter — an identifier, not an amount; left alone, same word-boundary reasoning as Pass 4)* |

Handled as its own pass rather than an override entry: a plain
text override can't tell a digit-adjacent `¢` apart from a bare one to
pick the right singular/plural form, or spell the preceding digit run out
itself. Must run before Pass 8 for the same "specific before generic"
reason as Pass 6d/7 — once this pass claims the digit run, Pass 8's
bare-number scan correctly skips the now non-raw chunk instead of
spelling the digits out again with no `cent(s)` suffix.

---

### Pass 6f — Ordinal numbers

A digit run immediately followed by `st`/`nd`/`rd`/`th` (case-insensitive,
no space) is expanded to its ordinal word form: the digit run is spelled
out the same digit-group way a bare cardinal would be
(`spell_digit_run`, shared with Pass 8), then only the last word of that
spelling is converted to its ordinal form.

| Input | Output |
|-------|--------|
| `15th` | `fifteenth` |
| `1st` | `first` |
| `2nd` | `second` |
| `3rd` | `third` |
| `12th` | `twelfth` |
| `22nd` | `twenty second` |
| `20th` | `twentieth` |
| `15TH` | `fifteenth` *(case-insensitive suffix match)* |
| `F15th` | `F15th` *(digit run glued to a preceding letter — an identifier, not a number; left alone, same word-boundary reasoning as Pass 4/6e)* |
| `15thing` | `15thing` *(a letter right after the suffix — not a standalone ordinal; left alone)* |

Found by ear: `"15th"` used to reach Pass 10's `split_alphanumeric`
(digit+letter token), which spelled only the digit run (`fifteen`) and left
the `th` suffix stranded as raw, unspoken letters — `"fifteen th"` instead
of `"fifteenth"`. No cross-check that the suffix "agrees" with the number
(e.g. `15st` still expands as if it read `15th`) — real documents are
assumed to use correct English ordinals, so this wasn't worth the added
complexity.

The last-word conversion (`cardinal_word_to_ordinal`) covers the irregular
cases (`one` → `first`, `two` → `second`, `three` → `third`, `five` →
`fifth`, `eight` → `eighth`, `nine` → `ninth`, `twelve` → `twelfth`,
`hundred` → `hundredth`, `thousand` → `thousandth`), the `y` → `ieth` tens
rule (`twenty` → `twentieth`, `thirty` → `thirtieth`), and falls back to a
plain `th` suffix for everything else (`four` → `fourth`, `six` → `sixth`,
`eleven` → `eleventh`, `thirteen` → `thirteenth`, ...). Must run before
Pass 8 for the same "specific before generic" reason as Pass 6d/6e/7 —
once this pass claims the digit run, Pass 8's bare-number scan and Pass
10's `split_alphanumeric` both correctly skip the now non-raw chunk
instead of spelling the digits out again with the suffix left bare.

---

### Pass 7 — Journal links

Patterns like `(AUGMENT,71279,)` or `<OAD,2237,>` — an optional bracket,
2+ uppercase letters, comma (optionally followed by whitespace), digits,
comma, optional closing bracket — are expanded: the name is title-cased,
the digits spelled digit-by-digit.

| Input | Output |
|-------|--------|
| `(AUGMENT,71279,)` | `Augment seven one two seven nine` |
| `(AUGMENT, 81010,)` | `Augment eight one zero one zero` |
| `<OAD,2237,>` | `O A D two two three seven` |
| `(DDD,xxx,bb)` | `(DDD,xxx,bb)` *(lowercase name — not matched; these are placeholder strings, not real links)* |

The whitespace tolerance after the first comma was added after a real
citation turned out typed with a space there,
unlike every other citation in the same document — the match used to
require the digit run glued directly to the comma, so this one citation
silently fell through unmatched, leaving a bare 5-digit number that, at
the time, no later pass could spell out (Pass 8 only covered 1-4 digit
runs; it has a 5+ digit fallback now, but this pass still must run first
— see below).

This pass must run *before* Pass 8 (the generic bare-number rule), not
just after Pass 6d — once Pass 8 grew its own 5+ digit fallback, a journal
link's digit run (`81010` above) would otherwise get claimed and spelled
out by Pass 8 before this pass ever saw `(AUGMENT, 81010,)` as one whole,
still-raw chunk to match against. Found via review, not a real document —
the same "more specific pass must run before a more generic one that
would otherwise shadow it" principle as Pass 6d before Pass 8, just
discovered the moment Pass 8's own coverage grew to overlap.

---

### Pass 8 — Bare number expansion

Any number not already handled by Pass 6d or Pass 7 gets spelled out, so
TTS reads it clearly and forced alignment (vocabulary is letters only) can
time-align it — a bare digit string is otherwise silently dropped by
alignment.

**Comma-grouped** numbers expand via `number_to_words` (the `num2words`
crate):

| Input | Output |
|-------|--------|
| `2,000` | `two thousand` |
| `100,000` | `one hundred thousand` |

**Bare 1-2 digit** numbers spell out directly:

| Input | Output |
|-------|--------|
| `5` | `five` |
| `0` | `zero` |
| `42` | `forty two` |

**Bare 3-digit** numbers read digit-group style (hundreds digit alone, then
the remaining two digits as a unit):

| Input | Output |
|-------|--------|
| `560` | `five sixty` |
| `501` | `five oh one` *(leading zero in the last two digits)* |
| `500` | `five hundred` *(round)* |

**Bare 4-digit** numbers read as two 2-digit groups, the way years and
addresses are normally spoken:

| Input | Output |
|-------|--------|
| `1976` | `nineteen seventy six` |
| `2010` | `twenty ten` |
| `2005` | `twenty oh five` *(leading zero in 2nd group)* |
| `1900` | `nineteen hundred` *(round hundred)* |
| `2000` | `two thousand` *(round thousand — `first group ends in 0`, reads as a single cardinal instead of "twenty hundred")* |

**Bare 5+ digit** numbers fall back to spelling each digit individually,
the same fallback used elsewhere in this file (Pass 3's `<Ref-N>` tags,
Pass 3b's footnote markers, Pass 4's numeric ranges) whenever a number is
too long for the digit-group convention above:

| Input | Output |
|-------|--------|
| `87574` | `eight seven five seven four` |
| `012345` | `zero one two three four five` *(leading zero doesn't matter here — digit-by-digit either way)* |

Found via a real document's bare ISBN component (`"ISBN 0-87574-297-3"`,
the `87574` segment once Pass 4's numeric-range chain scan correctly left
the whole ISBN alone — see Pass 4's "chain length" note above): previously
a bare 5+ digit number passed through completely unspoken, and since it
has no letters, it was invisible to forced alignment too — the exact
failure mode this whole file exists to avoid, just never closed for this
one length range.

**Excluded** (left bare, handled elsewhere):
- Leading-zero runs of length 2-4 (`05`, `0609`) — IDs, spelled
  digit-by-digit in Pass 10 instead (a 5+ digit leading-zero run is caught
  by the fallback above, same as any other 5+ digit run).
- A digit run touching a letter on either side (`4b`, `14B`, `v2`) — an
  alphanumeric ID, not a bare number; `split_alphanumeric` (Pass 10) spaces
  these out instead.

---

### Pass 8b — Rate units

Expands a `word/unit` rate expression to `word per unit` — only when the
word right after the `/` is one of a fixed table of rate units
(case-insensitive, singular or plural, always mapped to a singular output):
`sec`/`secs` → `second`, `min`/`mins` → `minute`, `hr`/`hrs` → `hour`, and
`day`/`week`/`month`/`year`/`decade` (already full words, just needing
`per` inserted) with their plurals.

| Input | Output |
|-------|--------|
| `cycles/sec` | `cycles per second` |
| `cycles/min` | `cycles per minute` |
| `bands/decade` | `bands per decade` |
| `samples / sec` | `samples per second` |
| `events/hrs` | `events per hour` |
| `and/or` | `and/or` *(unchanged — "or" isn't a rate unit)* |
| `his/her` | `his/her` *(unchanged)* |
| `document/file` | `document/file` *(unchanged)* |
| `miles/gallon` | `miles/gallon` *(unchanged — "gallon" isn't in the table)* |

Found via a real corpus's `"cycles/sec"`/`"bands/decade"` style rate
expressions: no earlier pass touches a bare `/`, so it reached TTS as
literal, unspoken punctuation — the same class of artifact Pass 0d fixes
for a bare colon. A blanket `/` → `" per "` rule was rejected: the same
corpus is full of ordinary word/word pairs using `/` for a completely
different purpose (`and/or`, `his/her`, `document/file`,
`human/computer`, `he/she`, `printer/graphic`) that would be mangled by a
generic rule. Restricting the match to a small, fixed rate-unit table on
the right-hand side is what disambiguates — none of those ordinary pairs'
right-hand words are in the table, so they're left alone without needing
their own exclusion list.

Two other slash forms in the same corpus are deliberately left alone by
this pass, both falling out naturally from requiring an actual
alphabetic word (not just a word boundary) immediately before the `/`:
- A DOI (`"10.1016/j.ijpsycho.2005.10.025"`) — the character right before
  the `/` is a digit, not a letter, so no left word is ever found there.
- Phonetic slash notation (`"the fricative /s/"`) — nothing but
  whitespace precedes the first `/`, and the bare letter `"s"` isn't a
  rate unit for the second, so neither slash matches.

A full URL (`"http://dx.doi.org/10.1111/ene.12296"`) is out of reach for
a different reason: Pass 0 replaces the whole URL with `link` before any
later pass, including this one, ever sees it.

---

### Pass 9 — US state abbreviations

Expands a 2-letter state postal code only in the `"City, ST"` position
(comma immediately before it) — not as a standalone-word override, since
many codes (`IN`, `OR`, `ME`, `HI`, `OK`, `OH`, ...) collide with common
English words; a flat word-list override would mangle those everywhere
they appear.

| Input | Output |
|-------|--------|
| `Denver, CO` | `Denver, Colorado` |
| `in Colorado` | `in Colorado` *(no comma — "CO" wouldn't even appear lowercase here)* |

A second, mixed-case form is also recognized: a capitalized letter,
lowercase letter, immediately followed by `.` — the informal way people
write the abbreviation (`Los Angeles, Ca.` → `Los Angeles, California.`).
Found via a real "Ca." artifact: the all-uppercase check doesn't match
mixed case, so it fell through untouched to Pass 10 as a stray short
token. The `.` is only a lookahead disambiguator, needed because mixed
case alone is too easily an ordinary word.

Whether that `.` is *also* consumed as part of the match, or left in
place, depends on what follows it — sentence splitting has already
happened by the time normalization runs, so a period followed by
whitespace + a lowercase letter means the splitter didn't treat it as a
sentence boundary (`"Ca"` isn't in the splitter's `ABBREVS` list, but
Unicode's own sentence-break heuristics don't fire on a lowercase
continuation either) — the whole thing arrived as one sentence chunk, and
this period is the abbreviation's own, not a real boundary. Consuming and
dropping it avoids planting a fake sentence break with a lowercase
continuation right after (`"...California. is my home."` reads as two
fractured sentences, wrong capitalization and all). Anything else after
it — whitespace + uppercase, other punctuation, or end of chunk — reads
as genuinely terminal, so it's left untouched instead.

| Input | Output |
|-------|--------|
| `Los Angeles, Ca.` | `Los Angeles, California.` *(end of chunk — terminal, kept)* |
| `Denver, Co. Today it rained.` | `Denver, Colorado. Today it rained.` *(uppercase follows — a real new sentence, kept)* |
| `San Francisco, Ca. is my home.` | `San Francisco, California is my home.` *(lowercase continuation — dropped)* |
| `Nowhere, Zz.` | `Nowhere, Zz.` *(not a real code — left alone)* |
| `Denver, Co.lorful` | `Denver, Co.lorful` *(letter right after the period — not a word boundary, left alone)* |

---

### Pass 10 — Tokenization

The text is split at non-alphanumeric boundaries. Each token is processed:

**a. Override lookup** — single-word match in the override table
(case-insensitive); replacement used as-is, no further rules apply.

**b. Alphanumeric splitting** — tokens mixing digits and letters have spaces
inserted between runs.

| Input | Output |
|-------|--------|
| `4b` | `4 b` |
| `4c2` | `4 c 2` |

**c. Roman numerals** — all-caps stems of 2+ letters are checked first;
lowercase Roman numerals are intentionally not handled (see Known
limitations). A 2-letter stem counts only if it uses just `I`/`V`/`X`;
pairs with `L`/`C`/`D`/`M` are far more often author initials than
numerals, so they fall through to rule d.

| Input | Output |
|-------|--------|
| `VIII` | `eight` |
| `IV` | `four` |
| `ML` | `M L` *(initials in a citation like `Oshinsky ML.`, not 1050)* |

**d. All-caps, short (≤3 letters, not a Roman numeral)** — spelled out
letter by letter, *unless* it's one of a fixed list of common short
function words (`SHORT_COMMON_WORDS`: articles, conjunctions,
prepositions, a few short pronouns/verbs — `the`, `to`, `for`, `in`, `of`,
`and`, `not`, `be`, etc.), in which case it's title-cased instead (same as
rule e below). An ALL-CAPS TITLE HEADING (`"THE END"`, `"TO BE OR NOT TO
BE"`) hits this branch just as easily as a real acronym does, and these
words are never acronyms.

| Input | Output |
|-------|--------|
| `UIS` | `U I S` |
| `TTS` | `T T S` |
| `THE` | `The` |
| `TO` | `To` |
| `NOT` | `Not` |

This check only fires when the token is genuinely all-caps in the source —
a normal mixed-case `"The"`/`"Not"` never reaches this branch at all. That
matters because an earlier attempt at this fix took a different approach:
case-insensitive override entries for a few of these words
directly (`the -> the`, `not -> not`, rule a above). Since a single-word
override matches regardless of the token's actual case, and its value is
used verbatim, that silently forced a normal sentence-initial
`"The"`/`"Not"` to lowercase too — believed to cause audible artifacts at
those sentence boundaries. Fixed at the `process_alpha_token` level
instead, gated on `all_caps`, so ordinary capitalization is never
touched.

**e. All-caps, long (>3 letters)** — title-cased (first letter capitalized,
rest lowercased) so the model reads them as a normal word rather than
letter-spelling them.

| Input | Output |
|-------|--------|
| `AUGMENT` | `Augment` |
| `AUGMENT's` | `Augment's` |

**f. Leading-zero sequences** — tokens starting with `0`, length > 1,
containing only digits, are spelled out digit-by-digit.

| Input | Output |
|-------|--------|
| `0609` | `zero six zero nine` |
| `069` | `zero six nine` |

**g. Remaining hyphens** — replaced with spaces, except a hyphen
immediately followed by a comma or by whitespace, which is dropped
outright instead of turned into a space — the following character already
supplies the separation, so converting the hyphen too would only stack a
redundant second one.

| Input | Output |
|-------|--------|
| `bit-mapped` | `bit mapped` |
| `on-line` | `on line` |
| `16-, 32- or 64-bit` | `sixteen, thirty two or sixty four bit` |

Found via a real document's elided-suffix list (`"16-, 32- or 64-bit
representation"`) — each digit is spelled out before this rule ever sees
it (`16` → `sixteen`, `32` → `thirty two`), so by the time Pass 10g runs,
each hyphen is glued directly onto either a comma (`sixteen-,`) or a space
(`thirty two- or`) with nothing meaningful between them. Turning either
into a space like any other hyphen left a stray extra space — before the
comma (`"sixteen , thirty two"`, the same TTS artifact Pass 0b exists to
avoid) or doubled up before "or" (`"thirty two  or"`). Pass 0b runs at the
very start of the pipeline, before either space exists, so it never gets a
second look. Scoped narrowly to a hyphen with nothing but a comma/space
immediately after it, so an ordinary hyphenated word like `64-bit` is
unaffected.

---

### Pass 11 — Strip brackets

Bracket characters `<`, `>`, `[`, `]`, `(`, `)` are removed (contents
kept). Prevents hallucinations on complex trailing punctuation.

If a bracket is glued directly to a word on **both** sides — no space or
other punctuation on either side — it's replaced with a single space
instead of just deleted, so the two words don't fuse into one unreadable
token.

| Input | Output |
|-------|--------|
| `for All Men(Philadelphia: Book` | `for All Men Philadelphia: Book` |
| `the Year 1782(Chester Springs` | `the Year seventeen eighty two Chester Springs` *(the digit expansion runs first — the "(" ends up glued to a word it wasn't glued to in the source)* |

Found via a real document (`for All Men(Philadelphia: Book & Publications
Committee,`) with no space before the parenthetical — the old plain
deletion turned it into `MenPhiladelphia`.

**Parenthetical asides — `(`/`)` become commas.** Unlike the other four
bracket characters, `(` and `)` get one more check before falling back to
plain deletion: if the bracket sits between real prose (the *outer*
neighbor — the word before `(`, or after `)` — is alphanumeric with no
pause punctuation already there) and the parenthetical's own content (the
*inner* neighbor, the char actually glued to the bracket, is also
alphanumeric), the bracket is replaced with a comma instead of just
deleted, mirroring how Pass 5 treats an unglued em-dash as an aside
boundary rather than dropping it silently.

| Input | Output |
|-------|--------|
| `Admiralty Arch (I had reached that monument), or` | `Admiralty Arch, I had reached that monument, or` |
| `see (Augment) now` | `see, Augment, now` *(neither side already has a pause — both brackets become commas)* |
| `monument), or` | `monument, or` *(the "," right after ")" is already a pause — plain deletion, not doubled up)* |
| `(Note this) is important` | `Note this, is important` *(the "(" opens the sentence — no outer neighbor, no leading comma; its ")" still independently qualifies)* |
| `word () other` | `word other` *(empty parens — inner neighbor isn't alphanumeric on either side, so both fall back to plain deletion)* |

Found via a real sentence, `"Admiralty Arch (I had reached that monument),
or any other avenue..."` — plain deletion left `"Arch I had"` with no
separator at all, and TTS read the bare `"I"` as a Roman-numeral/ordinal
suffix (`"Arch the First"`). `<`, `>`, `[`, `]` are unaffected by this
check and stay on the plain-deletion/space behavior above.

---

### Pass 12 — Capitalize sentence start

Capitalizes the first character of the normalized output if it's a
lowercase letter — unconditionally, whether the lowercase opener is the
original source text or a pass's output.

| Input reaching Pass 12 | Output |
|-------|--------|
| `sequel is a language` *(from `sql` → `sequel`, a deliberately-lowercase override, mid-pipeline)* | `Sequel is a language` |
| `all rights reserved` *(from `all` → `all`, an override that avoids letter-spelling `ALL`)* | `All rights reserved` |
| `see Item seven one two seven nine in the report` | `See Item seven one two seven nine in the report` |

Found via a real single-word override (`all` → `all`, added to stop `ALL`
being letter-spelled `A L L`): used verbatim, it lowercased the sentence's
opening word whenever `ALL`/`All` happened to start a sentence, which reads
oddly and has been observed to cause TTS artifacts. The fix is
unconditional rather than scoped to override-produced text specifically —
this normalized text is only ever heard via TTS, never shown to a reader,
so there's no orthographic reason to preserve a deliberately-lowercase
original spelling (e.g. a stylized brand name like `iPhone`) here; only
pronunciation matters, and it sounds the same either way (same reasoning as
the existing all-caps title-casing in Pass 10e).

---

## Known limitations

- **Context-dependent abbreviations** — `Dr.` always expands to `Doctor`.
  Drive vs. Doctor is not handled.
- **Uneven quote stripping** — `strip_short_quotes` removes quotes around
  ≤5-word phrases but does not track open/close balance; mismatched quotes
  can result in malformed text (e.g. quoted command-line strings).
- **Lowercase Roman numerals** — not expanded; see Pass 10c. Disambiguating
  "xiv" (= 14) from a lowercase placeholder string (e.g. "xxx" in
  `(DDD,xxx,bb)`) isn't reliably possible without per-document context, so
  a document that uses them gets per-document overrides (`ii` → "two",
  `(i)` → "one,"; a bare `i` can't be a key, since it would also replace
  the pronoun).

---

# Implementation

## Why span-mapping, not diffing

Forced alignment needs to map word timestamps — which land on *normalized*
text — back to character offsets in the *original* sentence text, where a
caller's own positions (such as annotations) are anchored.

The normalizer produces this mapping as a byproduct of normalization
itself, rather than reconstructing it after the fact by diffing normalized
text against the original. Diffing is fragile: it silently breaks whenever
a normalization rule changes the shape of its output (e.g. adding a new
case, reordering passes), since the diff algorithm has no notion of which
text came from which transformation. A forward mapping — built by the same
code that does the transforming — stays correct by construction; if a pass
is wrong, its mapping is wrong too, but at least consistently and visibly
wrong rather than silently misaligned.

## The `Spanned` chunk model

`normalize_with_spans` threads a `Vec<Spanned>` through all 13 passes,
where each `Spanned { text: String, src: Range<usize> }` is a chunk of
*normalized* text tagged with the char range in the *original* input it
derives from. `src` always anchors to the **original** input, never an
intermediate pass's output — that's what lets spans compose correctly
across multiple passes without each pass needing to know about any other.

`is_raw(chunk)` — `chunk.text.chars().count() == chunk.src.len()` — is true
only if no earlier pass has expanded or contracted the chunk's text. Every
scan-and-subdivide pass checks this and returns a non-raw chunk unchanged
rather than re-scanning into it, since local char offsets stop
corresponding to source offsets once a chunk has been expanded.

`NormalizedText::source_range(normalized_range)` is the public lookup: give
it a char range in the final normalized text, get back the corresponding
range in the original input (or `None` if out of bounds).

### Granularity is per-chunk, not per-word

A chunk that gets expanded (e.g. `<Ref-3>` → `Ref three`, or `Item 71279` →
`Item seven one two seven nine`) maps as a **whole** back to its source
range — there's no attempt to map individual output words within an
expansion back to individual input characters. This is why Pass 3 has to
spell its own trailing digits (via `spell_bare_digit_group`, the same helper
Pass 4 uses for year ranges) rather than leaving them bare for Pass 8 to
find later: once `<Ref-3>` becomes the non-raw chunk `Ref three`, Pass 8's
bare-number scan correctly skips over it — there is no later for digits
inside an already-expanded chunk.

So every output word inside one expanded chunk (all six of "Item"/"seven"/
"one"/"two"/"seven"/"nine") maps back to the same source range. A caller
mapping aligned words back to original text has to treat them as one unit,
not as six words with six positions.

`NormalizedText::source_words(spoken)` does that mapping for a list of
words, such as the words a forced aligner timed:

- It matches each word, case-insensitively, against the whole
  whitespace-separated words of the normalized text, searching forward
  from the previous match. Words the aligner dropped are skipped, and a
  short word never matches inside a longer one ("I" inside "detail").
- A word's range is the union of its chars' sources. A raw chunk maps
  char for char, so prose maps exactly; a transformed chunk maps every
  char to its whole source.
- A word can straddle chunks: "Winchester," is the raw "Winchester" plus
  the start of the expanded ", Massachusetts", so it maps to
  "Winchester, MA".
- To get one entry per source span, merge words whose ranges overlap.

## Three bug categories found while converting passes

Converting all 13 passes from flat-string transforms to chunk-aware ones
surfaced three recurring correctness traps, worth knowing before adding a
14th pass:

1. **Stale local-offset math on non-raw chunks.** A scan-and-subdivide pass
   that doesn't check `is_raw` first will recompute `base + local_offset`
   against a chunk whose local offsets no longer match source offsets,
   producing wrong spans. Fix: always check `is_raw` and return non-raw
   chunks unchanged.

2. **Naive length-changing "uniform map" passes.** A pass that changes a
   chunk's text length without subdividing it (e.g. an early em-dash → `, `
   implementation, or bracket-stripping done as a single string replace)
   silently breaks the `is_raw` invariant for the **whole** chunk,
   including unrelated trailing text — hiding that trailing text from every
   later pass. Fixed via a generic `replace_literal_spanned` /
   `replace_literal_chunk` helper that does proper flush-then-subdivide
   instead of a flat string replace.

3. **Patterns that can legitimately span chunk boundaries.** `Pass 2`
   (`strip_short_quotes`) needs to find quote pairs that can end up split
   across multiple chunks, because Pass 1 (`apply_punctuated_overrides`)
   runs first and can split text mid-quote-pair via an override match. A
   per-chunk-independent scan can't see across that split. This is the one
   pass in the whole pipeline that needs a global scan instead: flatten
   once, find removal positions in flattened-text coordinates, then
   surgically delete just those positions from whichever original chunks
   they land in. Documented here as a known general limitation — if a
   future pass needs to match a pattern that an earlier pass might split
   across chunks, it'll need the same approach, not a per-chunk scan.

## Pipeline ordering matters more than it looks

Pass 6d (Item/Ref digit-by-digit) must run before Pass 8 (generic bare
numbers) — not just any order: the more *specific* rule has to claim its
match and mark the chunk non-raw before the more *generic* rule's scan
would otherwise have reached the same text. Reordering these the other way
silently breaks Pass 6d's ID-style reading (e.g. "Item 1000" → "Item one
thousand" instead of the intended "Item one zero zero zero") with no
compiler error — only a behavioral regression, caught by tests. When
adding a new pass whose pattern overlaps an existing one, place the more
specific pass first.

A related but distinct trap: a pass that runs *before* Pass 8 and produces
its own digit content (Pass 4, numeric ranges) must spell that content out
itself rather than leaving it bare for Pass 8 to "clean up later" — there
is no later for digits inside an already-expanded chunk, since Pass 8's
scan skips non-raw chunks entirely (`is_raw`). This was missed initially
for Pass 4 (it only replaced the hyphen with "to", leaving both digit runs
bare) and wasn't just a pronunciation gap: bare digits have no letters, so
forced alignment dropped them from its output entirely, and a word lookup
for a year range (e.g. `1973-76`) found *no* match at all rather than a
slightly-off one. Fixed by extracting `spell_bare_digit_group` (shared by Pass 4 and Pass 8) so Pass 4 spells
both halves itself before marking the chunk non-raw.

A third variant, found the moment Pass 8's own coverage grew rather than
from a real document: Pass 7 (journal links) and Pass 8 (generic bare
numbers) used to be numbered the other way around — Pass 8 ran first, and
that was safe only because Pass 8 couldn't reach a bare 5+ digit number at
all yet. The moment Pass 8 grew a digit-by-digit fallback for exactly that
case (to close the previous limitation above), it started reaching *into*
what a journal link like `(AUGMENT, 81010,)` needed to still see as one
whole, untouched chunk — Pass 8 would spell out `81010` on its own before
Pass 7 (the more specific match) ever got a look at the surrounding
`(AUGMENT, ...,)` shape. Fixed by swapping their order (and, to keep this
file's own "pass number = execution order" rule intact, their numbers)
rather than trying to make Pass 8's new fallback specifically avoid
journal-link-shaped text — same "more specific pass first" principle as
Pass 6d/Pass 8 above, just triggered by a later change growing one pass's
reach into territory a different, already-correct pass depended on staying
untouched.

A third variant of the same trap, found converting real-world documents:
identifier-dot replacement (now Pass 6c) originally ran *last* (old Pass
10), after Pass 6d/7 had already spelled out any digit runs on either side
of a `.` into separate non-raw chunks. That stranded the `.` as an isolated
1-char raw chunk with no adjacent text left within its own chunk for the
dot pass's local `chars[i+1]` lookahead to see — `26.44` normalized to
`twenty six.forty four` (the `.` surviving bare and glued, no spaces on
either side), and forced alignment saw `six.forty` as one ungapped token
rather than two separate words. Unlike the Pass 4 case, the fix here wasn't
to make the *later*-running pass spell its own digits — it was to move the
dot pass *earlier*, before Pass 6d/7 touch the digits at all. That also
made the `point`-vs-`dot` decision trivial: with digits still raw, Pass 6c
can check `chars[i-1]`/`chars[i+1]` directly instead of needing to peek
into already-transformed neighbor chunks or thread the original source
string through for a lookup.

Pass 6b (number sign) is the same lesson applied on sight rather than
learned the hard way: `#26.44` (a recurring paragraph.item citation
convention in this document) glues a bare `#` directly onto a digit run
exactly like the dot case, so it was placed before Pass 6c/d/7 from the
start rather than added at the end and reordered later.

A fourth variant, the currency pass itself: `£`/`$` amounts glue their
symbol directly onto a digit run the same way, and a decimal amount
(`$45.50`) glues the fractional part on with a `.` exactly like Pass 6c's
identifier-dot case — so Pass 6a runs before Pass 6c for the same reason,
claiming the whole `$45.50` shape (symbol, whole part, and fractional
part) as one match before the dot pass ever gets a chance to turn the `.`
into `" point "`.
