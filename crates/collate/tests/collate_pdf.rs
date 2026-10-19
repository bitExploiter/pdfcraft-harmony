//! What PdfCraft needs on top of the collation: natural (number-aware) order for file names,
//! whole-letter search with the viewer's Turkish and Greek folds, and language tags.

use std::cmp::Ordering;

use pdfcraft_collate::{Collation, compare, natural, search_contains, search_contains_folded, search_fold, text_key};

#[test]
fn natural_numbers_by_value_letters_by_collation() {
    let nfd10 = "Ba\u{301}o ca\u{301}o 10.pdf";
    let nfc2 = "Báo cáo 2.pdf";
    assert_eq!(natural(nfc2, nfd10, Collation::Root), Ordering::Less);
    assert_eq!(natural("scan2", "scan10", Collation::Root), Ordering::Less);
    assert_eq!(natural("a007", "a7", Collation::Root), Ordering::Equal); // like upstream: leading zeros are dropped
    assert_eq!(natural("Đà Nẵng.pdf", "Dung.pdf", Collation::Vi), Ordering::Greater);
    assert_eq!(natural("Đà Nẵng.pdf", "Dung.pdf", Collation::Root), Ordering::Less);
    let long = format!("x{}", "9".repeat(400));
    assert_eq!(natural(&long, "x10", Collation::Root), Ordering::Greater);
}

#[test]
fn natural_edge_cases() {
    for c in [Collation::Root, Collation::Vi] {
        assert_eq!(natural("", "", c), Ordering::Equal);
        assert_eq!(natural("", "a", c), Ordering::Less);
        assert_eq!(natural("a", "", c), Ordering::Greater);
        assert_eq!(natural("a", "a1", c), Ordering::Less); // the string that ends first is smaller
        assert_eq!(natural("a1", "a", c), Ordering::Greater);
        assert_eq!(natural("1", "a", c), Ordering::Less); // a digit run against a letter run: digits first
        assert_eq!(natural("file2b", "file2a", c), Ordering::Greater);
        assert_eq!(natural("file02b", "file2a", c), Ordering::Greater);
        assert_eq!(natural("a0", "a00", c), Ordering::Equal);
        assert_eq!(natural("a0", "a1", c), Ordering::Less);
        // Case and encoding do not matter in the letter runs.
        assert_eq!(natural("Báo 2", "BA\u{301}O 2", c), Ordering::Equal);
        // Only ASCII digits are number runs.
        assert_eq!(natural("a\u{661}", "a\u{662}", c), compare("a\u{661}", "a\u{662}", c));
    }
}

#[test]
fn natural_is_a_consistent_order() {
    let words = [
        "",
        "a",
        "a1",
        "a2",
        "a10",
        "a010",
        "A1",
        "Á1",
        "A\u{301}1",
        "a1b",
        "a1b2",
        "b",
        "scan9",
        "scan10",
        "scan010",
        "Đà 2",
        "Da 10",
        "1",
        "01",
        "10",
    ];
    for c in [Collation::Root, Collation::Vi] {
        for a in words {
            for b in words {
                assert_eq!(natural(a, b, c), natural(b, a, c).reverse(), "{a:?} {b:?} {c:?}");
                for d in words {
                    if natural(a, b, c) != Ordering::Greater && natural(b, d, c) != Ordering::Greater {
                        assert_ne!(natural(a, d, c), Ordering::Greater, "{a:?} {b:?} {d:?} {c:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn search_keeps_turkish_and_sigma_and_whole_letters() {
    assert!(search_contains("İSTANBUL ŞUBESİ", "istanbul"));
    assert!(search_contains("Kış dönemi", "KIŞ"));
    assert!(search_contains("ΟΔΟΣ", "οδος"));
    assert!(search_contains("Chu\u{31b}o\u{31b}ng 1", "Chương"));
    assert!(search_contains("NGUYE\u{302}\u{303}N", "nguyễn"));
    assert!(!search_contains("Nguyễn", "e"));
    assert!(search_contains("anything", ""));
}

#[test]
fn search_fold_maps_like_the_viewer_then_folds_the_key() {
    // The mapping of `pdfcraft_render::text::fold_case`: I i İ ı -> i, Σ σ ς -> σ.
    assert_eq!(search_fold("I\u{130}\u{131}i"), "iiii");
    assert_eq!(search_fold("Σσς"), "σσσ");
    // Then the key: lowercase, decomposed, marks in canonical order.
    assert_eq!(search_fold("NGUYỄN"), search_fold("nguye\u{302}\u{303}n"));
    assert_eq!(search_fold("Ánh"), "a\u{301}nh");
    // The folded form searches without folding again; overlapping occurrences are all tried.
    assert!(search_contains_folded(&search_fold("ễ e"), &search_fold("e")));
    assert!(!search_contains_folded(&search_fold("ễ"), &search_fold("e")));
    assert!(search_contains_folded("anything", ""));
    assert!(search_contains_folded("", ""));
    assert!(!search_contains_folded("", "a"));
}

#[test]
fn tags_map_to_collations() {
    for t in ["vi", "vi-VN", "VI_vn", " vi "] {
        assert_eq!(Collation::from_tag(t), Collation::Vi, "{t:?}");
    }
    for t in ["", "-", "zz", "en-US", "vie", "v"] {
        assert_eq!(Collation::from_tag(t), Collation::Root, "{t:?}");
    }
    assert_eq!(Collation::Root.tag(), "root");
    assert_eq!(Collation::Vi.tag(), "vi");
}

#[test]
fn hostile_text_never_panics() {
    let marks = "a".to_string() + &"\u{323}\u{302}".repeat(5_000); // 10 000 combining marks
    let leading = "\u{301}\u{323}\u{302}a".to_string(); // marks first
    let big = "ệ".repeat(350_000); // 1 MB (3 bytes each)
    let digits = format!("x{}y{}", "7".repeat(50_000), "9".repeat(50_000));
    let cases: [&str; 7] = ["", "\u{10FFFF}", &marks, &leading, &big, &digits, "\u{301}"];
    for c in [Collation::Root, Collation::Vi] {
        for a in cases {
            let ka = text_key(a, c);
            for b in cases {
                let o = compare(a, b, c);
                assert_eq!(o, compare(b, a, c).reverse());
                assert_eq!(ka.cmp(&text_key(b, c)), o);
                assert_eq!(natural(a, b, c), natural(b, a, c).reverse());
                let _ = search_contains(a, b);
            }
        }
    }
}
