//! Text matching for the search boxes: one fold, so "uloha" finds "Úloha"
//! and "zluty" finds "žltý" wherever a person types a query.
//!
//! The fold lower-cases, drops combining marks and maps the Latin-1 and
//! Latin Extended-A letters (U+00C0–U+017F) to their bare ASCII letter.
//! It is a table, not Unicode decomposition, so the desktop
//! (`src/lib/text_fold.ts`) and the mobile app fold the same way; the
//! table is the same string in all three.

/// U+00C0..U+0180, lower-cased: the bare letter, or `.` for a letter kept
/// as it is (æ, ß, œ, ĳ, the multiplication and division signs).
const LATIN: &[u8; 192] = b"aaaaaa.ceeeeiiiidnooooo.ouuuuy..aaaaaa.ceeeeiiiidnooooo.ouuuuy.y\
aaaaaaccccccccddddeeeeeeeeeegggggggghhhhiiiiiiiiii..jjkkklllllll\
lllnnnnnnn..oooooo..rrrrrrssssssssttttttuuuuuuuuuuuuwwyyyzzzzzzs";

/// `s` lower-cased with its diacritics dropped.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        if ('\u{300}'..='\u{36f}').contains(&c) {
            continue;
        }
        let cp = c as u32;
        if (0xC0..0x180).contains(&cp) {
            let b = LATIN[(cp - 0xC0) as usize];
            if b != b'.' {
                out.push(b as char);
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Whether every word of `query` is in `folded` (already [`fold`]ed). An
/// empty query matches everything.
pub fn matches_words(folded: &str, query: &str) -> bool {
    fold(query).split_whitespace().all(|w| folded.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_and_diacritics() {
        assert_eq!(fold("Úloha ŽLTÝ kôň"), "uloha zlty kon");
        assert_eq!(fold("Łódź Straße Ærø"), "lodz straße æro");
        // A decomposed "é" (e + U+0301) folds like the precomposed one.
        assert_eq!(fold("Cafe\u{301}"), "cafe");
        assert_eq!(fold("İstanbul"), "istanbul");
    }

    #[test]
    fn every_query_word_must_be_present_in_any_order() {
        let hay = fold("ABC-12 Oprava prihlásenia cez SSO");
        assert!(matches_words(&hay, "sso prihlasenia"));
        assert!(matches_words(&hay, "  abc-12  "));
        assert!(matches_words(&hay, ""));
        assert!(!matches_words(&hay, "sso odhlasenie"));
    }

    #[test]
    fn the_table_is_the_one_the_clients_carry() {
        let ts = crate::repo_files::read("src/lib/text_fold.ts");
        // The TS file splits it into three 64-letter lines, as here.
        for line in LATIN.chunks(64) {
            let line = std::str::from_utf8(line).unwrap();
            assert!(
                ts.contains(line),
                "src/lib/text_fold.ts lacks the fold line {line}"
            );
        }
    }
}
