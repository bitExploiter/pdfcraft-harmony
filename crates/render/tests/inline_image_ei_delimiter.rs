//! Synthetic content-stream regressions for #3578. These exercise the renderer's actual lexer
//! from a workspace test: vendor/hayro-syntax's own unit tests are excluded from workspace CI.

use hayro_syntax::content::{TypedIter, UntypedIter, ops::TypedInstruction};
use hayro_syntax::object::Object;

/// Construct an inline image followed by content, without any external PDF assets.
fn content(parameters: &str, samples: &[u8], suffix: &[u8]) -> Vec<u8> {
    let mut data = format!("BI {parameters} ID ").into_bytes();
    data.extend_from_slice(samples);
    data.extend_from_slice(b" EI");
    data.extend_from_slice(suffix);
    data
}

/// Check the bytes retained by the untyped lexer and the text reached by typed dispatch.
fn assert_image_and_text(data: &[u8], samples: &[u8], following: &[&[u8]]) {
    let mut iter = UntypedIter::new(data);
    let image = iter.next().expect("BI instruction");
    assert_eq!(image.operator.as_ref(), b"BI");
    let stream = image.operands().find_map(|operand| match operand {
        Object::Stream(stream) => Some(stream),
        _ => None,
    });
    let stream = stream.expect("inline image stream operand");
    // The heuristic currently retains the whitespace immediately before EI. Preserve that API.
    let mut expected = samples.to_vec();
    expected.push(b' ');
    assert_eq!(stream.raw_data().as_ref(), expected.as_slice());

    let mut operators = Vec::new();
    while let Some(instruction) = iter.next() {
        operators.push(instruction.operator.as_ref().to_vec());
        assert!(operators.len() <= data.len(), "lexer must make progress");
    }
    assert_eq!(operators.iter().map(Vec::as_slice).collect::<Vec<_>>(), following);

    let mut typed = TypedIter::new(data);
    let mut shown = Vec::new();
    let mut steps = 0;
    while let Some(instruction) = typed.next() {
        steps += 1;
        assert!(steps <= data.len(), "typed iterator must make progress");
        if let TypedInstruction::ShowText(text) = instruction {
            shown.push(text.0.as_bytes().to_vec());
        }
    }
    assert_eq!(shown, [b"AFTER".to_vec()]);
}

#[test]
fn inline_image_ei_accepts_comments_arrays_and_whitespace() {
    let text_ops: &[&[u8]] = &[b"BT", b"Tj", b"ET"];
    for suffix in [
        b"%comment\nBT (AFTER) Tj ET".as_slice(),
        b" BT (AFTER) Tj ET",
        b"\tBT (AFTER) Tj ET",
        b"\nBT (AFTER) Tj ET",
        b"\rBT (AFTER) Tj ET",
        b"\x0cBT (AFTER) Tj ET",
        b"\0BT (AFTER) Tj ET",
    ] {
        let data = content("/W 1 /H 1 /BPC 8 /CS /G", b"\x80", suffix);
        assert_image_and_text(&data, b"\x80", text_ops);
    }
    let data = content("/W 1 /H 1 /BPC 8 /CS /G", b"\x80", b"[] 0 d BT (AFTER) Tj ET");
    assert_image_and_text(&data, b"\x80", &[b"d", b"BT", b"Tj", b"ET"]);

    // Exercise the filtered path too; a known unfiltered byte count cannot mask this bug.
    for suffix in [b"%comment\nBT (AFTER) Tj ET".as_slice(), b"[] 0 d BT (AFTER) Tj ET"] {
        let data = content("/W 1 /H 1 /BPC 8 /CS /G /F /A85", b"J,~>", suffix);
        let following: &[&[u8]] = if suffix.starts_with(b"[") { &[b"d", b"BT", b"Tj", b"ET"] } else { text_ops };
        assert_image_and_text(&data, b"J,~>", following);
    }
}

#[test]
fn inline_image_false_ei_candidates_keep_the_image_whole() {
    for samples in [
        // Q is a regular character, so EIQ cannot be an EI operator.
        b"abEIQcd".as_slice(),
        // A legal suffix is only a candidate: the binary look-ahead must still reject this EI.
        b"ab EI%fake\n\x80cd",
        b"ab EI \x80cd",
    ] {
        let parameters = format!("/W {} /H 1 /BPC 8 /CS /G", samples.len());
        let data = content(&parameters, samples, b"%comment\nBT (AFTER) Tj ET");
        assert_image_and_text(&data, samples, &[b"BT", b"Tj", b"ET"]);
    }

    // '[' and ']' are ASCII85 digits as well as PDF delimiters. The candidate inside this
    // valid six-sample encoding must fail the existing typed-content look-ahead.
    let samples = b"EI[]!!!!~>";
    let data = content("/W 6 /H 1 /BPC 8 /CS /G /F /A85", samples, b"[] 0 d BT (AFTER) Tj ET");
    assert_image_and_text(&data, samples, &[b"d", b"BT", b"Tj", b"ET"]);
}

#[test]
fn inline_image_ei_at_eof_and_glued_filter_end_still_work() {
    let data = content("/W 1 /H 1 /BPC 8 /CS /G", b"\x80", b"");
    let mut iter = UntypedIter::new(&data);
    let image = iter.next().expect("BI at EOF");
    assert!(image.operands().any(|operand| matches!(operand, Object::Stream(stream) if stream.raw_data().as_ref() == b"\x80 ")));
    assert!(iter.next().is_none());

    // Keep hayro's existing acceptance of ~>EI and >EI without preceding whitespace (#3108).
    for data in [
        b"BI /W 1 /H 1 /BPC 8 /CS /G /F /A85 ID J,~>EI%comment\nBT (AFTER) Tj ET".as_slice(),
        b"BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 80>EI%comment\nBT (AFTER) Tj ET",
    ] {
        let mut typed = TypedIter::new(data);
        let mut shown = Vec::new();
        while let Some(instruction) = typed.next() {
            if let TypedInstruction::ShowText(text) = instruction {
                shown.push(text.0.as_bytes().to_vec());
            }
        }
        assert_eq!(shown, [b"AFTER".to_vec()]);
    }
}

#[test]
fn truncated_inline_images_finish_without_panicking() {
    for data in [
        b"BI /W 1 /H 1 /BPC 8 /CS /G ID \x80 EI%comment\nBT (AFTER) Tj ET".as_slice(),
        b"BI /W 6 /H 1 /BPC 8 /CS /G /F /A85 ID EI[]!!!!~> EI[] 0 d BT (AFTER) Tj ET",
        b"BI /W 1 /H 1 /BPC 8 /CS /G ID \x80 EIQ",
        b"BI /W 1 /H 1 /BPC 8 /CS /G ID \x80", // No end marker.
    ] {
        // Includes empty input, truncated dictionaries, ID, E, EI, comments and arrays.
        for end in 0..=data.len() {
            let mut iter = UntypedIter::new(&data[..end]);
            let mut steps = 0;
            while iter.next().is_some() {
                steps += 1;
                assert!(steps <= end + 1, "lexer must finish for prefix {end}");
            }
            let mut typed = TypedIter::new(&data[..end]);
            let mut steps = 0;
            while typed.next().is_some() {
                steps += 1;
                assert!(steps <= end + 1, "typed iterator must finish for prefix {end}");
            }
        }
    }
}
