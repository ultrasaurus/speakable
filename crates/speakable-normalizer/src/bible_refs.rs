//! Bible book name/abbreviation matching for normalizer.rs Pass 3c
//! ("Bible verse references"). Kept in its own module since the book table
//! is long and would otherwise dominate normalizer.rs.

/// One book: its full written name (used for matching source text), the
/// spoken form of that name (used in the expanded output — differs from
/// `full_name` only for a numbered book, where the leading digit is spoken
/// as an ordinal, e.g. "1 Corinthians" -> "First Corinthians"), the
/// abbreviations that count as a match (each requiring a trailing period at
/// the call site — this table stores the bare abbreviation text only), and
/// whether it uses "Psalm"-style wording (no "chapter" word) rather than
/// the default "chapter N verse M".
struct Book {
    full_name: &'static str,
    spoken_name: &'static str,
    abbrevs: &'static [&'static str],
    psalms_style: bool,
}

macro_rules! book {
    ($full:expr, [$($abbrev:expr),* $(,)?]) => {
        Book { full_name: $full, spoken_name: $full, abbrevs: &[$($abbrev),*], psalms_style: false }
    };
    ($full:expr, spoken: $spoken:expr, [$($abbrev:expr),* $(,)?]) => {
        Book { full_name: $full, spoken_name: $spoken, abbrevs: &[$($abbrev),*], psalms_style: false }
    };
}

static BOOKS: &[Book] = &[
    book!("Genesis", ["Gen.", "Ge.", "Gn."]),
    book!("Exodus", ["Ex.", "Exod.", "Exo."]),
    book!("Leviticus", ["Lev.", "Le.", "Lv."]),
    book!("Numbers", ["Num.", "Nu.", "Nm.", "Nb."]),
    book!("Deuteronomy", ["Deut.", "De.", "Dt."]),
    book!("Joshua", ["Josh.", "Jos.", "Jsh."]),
    book!("Judges", ["Judg.", "Jdg.", "Jg.", "Jdgs."]),
    book!("Ruth", ["Ruth.", "Rth.", "Ru."]),
    book!("1 Samuel", spoken: "First Samuel", ["1 Sam.", "1 Sm.", "1 Sa.", "1Sam."]),
    book!("2 Samuel", spoken: "Second Samuel", ["2 Sam.", "2 Sm.", "2 Sa.", "2Sam."]),
    book!("1 Kings", spoken: "First Kings", ["1 Kgs.", "1 Ki.", "1K.", "1Kgs."]),
    book!("2 Kings", spoken: "Second Kings", ["2 Kgs.", "2 Ki.", "2K.", "2Kgs."]),
    book!("1 Chronicles", spoken: "First Chronicles", ["1 Chron.", "1 Chr.", "1 Ch.", "1Chron."]),
    book!("2 Chronicles", spoken: "Second Chronicles", ["2 Chron.", "2 Chr.", "2 Ch.", "2Chron."]),
    book!("Ezra", ["Ezra.", "Ezr.", "Ez."]),
    book!("Nehemiah", ["Neh.", "Ne."]),
    book!("Esther", ["Esth.", "Est.", "Es."]),
    book!("Job", ["Job.", "Jb."]),
    book!("Proverbs", ["Prov.", "Pro.", "Prv.", "Pr."]),
    book!("Ecclesiastes", ["Eccles.", "Eccl.", "Ecc.", "Ec."]),
    book!("Song of Solomon", ["Song.", "SS.", "Song of Sol.", "Canticles."]),
    book!("Isaiah", ["Isa.", "Is."]),
    book!("Jeremiah", ["Jer.", "Je.", "Jr."]),
    book!("Lamentations", ["Lam.", "La."]),
    book!("Ezekiel", ["Ezek.", "Eze.", "Ezk."]),
    book!("Daniel", ["Dan.", "Da.", "Dn."]),
    book!("Hosea", ["Hos.", "Ho."]),
    book!("Joel", ["Joel.", "Jl."]),
    book!("Amos", ["Amos.", "Am."]),
    book!("Obadiah", ["Obad.", "Ob."]),
    book!("Jonah", ["Jonah.", "Jnh.", "Jon."]),
    book!("Micah", ["Mic.", "Mc."]),
    book!("Nahum", ["Nah.", "Na."]),
    book!("Habakkuk", ["Hab.", "Hb."]),
    book!("Zephaniah", ["Zeph.", "Zep.", "Zp."]),
    book!("Haggai", ["Hag.", "Hg."]),
    book!("Zechariah", ["Zech.", "Zec.", "Zc."]),
    book!("Malachi", ["Mal.", "Ml."]),
    book!("Matthew", ["Matt.", "Mt."]),
    book!("Mark", ["Mark.", "Mrk.", "Mk.", "Mr."]),
    book!("Luke", ["Luke.", "Lk."]),
    book!("John", ["John.", "Jn.", "Jhn."]),
    book!("Acts", ["Acts.", "Ac."]),
    book!("Romans", ["Rom.", "Ro.", "Rm."]),
    book!("1 Corinthians", spoken: "First Corinthians", ["1 Cor.", "1 Co.", "1Cor."]),
    book!("2 Corinthians", spoken: "Second Corinthians", ["2 Cor.", "2 Co.", "2Cor."]),
    book!("Galatians", ["Gal.", "Ga."]),
    book!("Ephesians", ["Eph.", "Ephes."]),
    book!("Philippians", ["Phil.", "Php."]),
    book!("Colossians", ["Col.", "Colo."]),
    book!("1 Thessalonians", spoken: "First Thessalonians", ["1 Thess.", "1 Th.", "1Thess."]),
    book!("2 Thessalonians", spoken: "Second Thessalonians", ["2 Thess.", "2 Th.", "2Thess."]),
    book!("1 Timothy", spoken: "First Timothy", ["1 Tim.", "1 Ti.", "1Tim."]),
    book!("2 Timothy", spoken: "Second Timothy", ["2 Tim.", "2 Ti.", "2Tim."]),
    book!("Titus", ["Titus.", "Tit.", "Ti."]),
    book!("Philemon", ["Philem.", "Phm.", "Pm."]),
    book!("Hebrews", ["Heb.", "Hebr."]),
    book!("James", ["James.", "Jas.", "Jm."]),
    book!("1 Peter", spoken: "First Peter", ["1 Pet.", "1 Pt.", "1Pet."]),
    book!("2 Peter", spoken: "Second Peter", ["2 Pet.", "2 Pt.", "2Pet."]),
    book!("1 John", spoken: "First John", ["1 John.", "1 Jn.", "1Jn."]),
    book!("2 John", spoken: "Second John", ["2 John.", "2 Jn.", "2Jn."]),
    book!("3 John", spoken: "Third John", ["3 John.", "3 Jn.", "3Jn."]),
    book!("Jude", ["Jude.", "Jud."]),
    book!("Revelation", ["Rev.", "Re.", "Rv."]),
    // Psalms: singular "Psalm" spoken form for a single-chapter reference,
    // no "chapter" word (e.g. "Psalm seventy three verse twenty one").
    Book { full_name: "Psalm", spoken_name: "Psalm", abbrevs: &["Ps.", "Psa.", "Psalm.", "Psalms."], psalms_style: true },
];

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// If a known book name or abbreviation starts at `chars[i]` (word boundary
/// before and after the match), returns the book's spoken name (a numbered
/// book's leading digit spoken as an ordinal, e.g. "First Corinthians"),
/// whether it's Psalms-style, and how many chars the match consumed.
/// Abbreviations are matched case-sensitively (title case only, same
/// convention as the state-abbreviation pass) and always include their
/// trailing period; a full name never has one. Longest match wins so a
/// prefix like "Ps." doesn't shadow "Psalms." — not that it matters here
/// since both map to the same book, but it keeps the rule general.
pub fn match_book(chars: &[char], i: usize) -> Option<(&'static str, bool, usize)> {
    if i > 0 && is_word_char(chars[i - 1]) { return None; }

    let mut best: Option<(&'static str, bool, usize)> = None;
    for book in BOOKS {
        for candidate in std::iter::once(book.full_name).chain(book.abbrevs.iter().copied()) {
            let cand_chars: Vec<char> = candidate.chars().collect();
            let len = cand_chars.len();
            if i + len > chars.len() { continue; }
            if chars[i..i + len] != cand_chars[..] { continue; }
            // Word boundary after the match — but a trailing "." in an
            // abbreviation is itself the boundary character, so only check
            // past it when the candidate doesn't already end in one.
            if !candidate.ends_with('.')
                && i + len < chars.len() && is_word_char(chars[i + len]) { continue; }
            if best.map(|(_, _, best_len)| len > best_len).unwrap_or(true) {
                best = Some((book.spoken_name, book.psalms_style, len));
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn try_match(s: &str) -> Option<(&'static str, bool, usize)> {
        let chars: Vec<char> = s.chars().collect();
        match_book(&chars, 0)
    }

    #[test]
    fn matches_abbrev_with_period() {
        assert_eq!(try_match("Mt. 5:30"), Some(("Matthew", false, 3)));
    }

    #[test]
    fn matches_full_name() {
        assert_eq!(try_match("Matthew 5:30"), Some(("Matthew", false, 7)));
    }

    #[test]
    fn matches_psalms_abbrev() {
        assert_eq!(try_match("Ps. 73:21"), Some(("Psalm", true, 3)));
    }

    #[test]
    fn matches_psalms_full_name() {
        assert_eq!(try_match("Psalm 73:21"), Some(("Psalm", true, 5)));
    }

    #[test]
    fn matches_numbered_book() {
        assert_eq!(try_match("1 Cor. 13:4"), Some(("First Corinthians", false, 6)));
    }

    #[test]
    fn no_match_without_word_boundary() {
        // "Mtn." is not a book abbreviation, and must not partially match "Mt."
        assert_eq!(try_match("Mtn. 5:30"), None);
    }

    #[test]
    fn no_match_for_unrelated_word() {
        assert_eq!(try_match("Meeting at 5:30"), None);
    }
}
