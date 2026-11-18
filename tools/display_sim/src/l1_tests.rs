use super::*;

fn csv(labels: std::ops::Range<u64>) -> String {
    let mut text = String::from("frame,min_pq,max_pq,avg_pq\n");
    for frame in labels {
        text.push_str(&format!("{frame},10,{}.5,{}\n", 3000 + frame, 1500 + frame));
    }
    text
}

fn rows(labels: std::ops::Range<u64>) -> Vec<L1Row> {
    parse_csv(&csv(labels)).unwrap()
}

#[test]
fn decimal_and_integer_codes_parse() {
    let rows =
        parse_csv("frame,min_pq,max_pq,avg_pq\n0,12,3078.73,1543\n\n1, 0 ,4095,0\n").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (
            rows[0].frame,
            rows[0].metadata.max_code,
            rows[0].metadata.avg_code
        ),
        (0, 3078.73, 1543.0)
    );
    assert_eq!(
        (
            rows[1].line,
            rows[1].metadata.max_code,
            rows[1].metadata.avg_code
        ),
        (4, 4095.0, 0.0)
    );
    // Without a header the first line is data.
    assert_eq!(parse_csv("7,1,2,3\n").unwrap()[0].frame, 7);
}

#[test]
fn malformed_rows_are_refused() {
    for (text, message) in [
        ("0,1,2\n", "has 3 columns"),
        ("0,1,4096,3\n", "max_pq 4096 is not a 12-bit PQ code"),
        ("0,1,2,-0.5\n", "avg_pq -0.5 is not"),
        ("0,-1,2,3\n", "min_pq -1 is not"),
        ("0,1,NaN,3\n", "max_pq NaN is not"),
        ("0,1,x,3\n", "max_pq 'x'"),
        ("a,1,2,3\n", "frame 'a'"),
    ] {
        let error = format!("{:#}", parse_csv(text).unwrap_err());
        assert!(error.contains(message), "{text:?}: {error}");
    }
}

#[test]
fn csvs_line_up_with_the_measured_frames() {
    // No leading pictures: labels from 0, one per frame.
    let stream = align(&rows(0..5), 5, 0).unwrap();
    assert_eq!(
        (stream.coordinates, stream.skipped, stream.metadata.len()),
        (Coordinates::Stream, 0, 5)
    );
    assert_eq!(stream.metadata[0].max_code, 3000.5);
    // Two leading pictures: a whole-stream CSV skips their rows ...
    let whole = align(&rows(0..7), 5, 2).unwrap();
    assert_eq!((whole.coordinates, whole.skipped), (Coordinates::Stream, 2));
    assert_eq!(whole.metadata[0].max_code, 3002.5);
    // ... an l1_diff export starts at the first measured frame ...
    let export = align(&rows(2..7), 5, 2).unwrap();
    assert_eq!(
        (export.coordinates, export.skipped),
        (Coordinates::Stream, 0)
    );
    assert_eq!(export.metadata, whole.metadata);
    // ... and a CSV labelled from 0 with one row per measured frame is in decoded coordinates.
    let decoded = align(&rows(0..5), 5, 2).unwrap();
    assert_eq!(decoded.coordinates, Coordinates::Decoded);
    assert_eq!(decoded.metadata[0].max_code, 3000.5);
}

#[test]
fn csvs_that_do_not_describe_the_dump_are_refused() {
    for (labels, frames, leading) in [
        (0..4, 5, 0), // a dump frame missing
        (0..6, 5, 0), // one row too many
        (1..6, 5, 0), // shifted
        (3..7, 5, 2), // starts after the first measured frame
        (0..6, 5, 2), // ends before the last stream frame
    ] {
        let error = align(&rows(labels.clone()), frames, leading)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("frame count mismatch"),
            "{labels:?}: {error}"
        );
    }
    let mut gap = rows(0..6);
    gap.remove(3);
    let error = align(&gap, 5, 0).unwrap_err().to_string();
    assert!(error.contains("frame 4 follows frame 2"), "{error}");
    let mut duplicate = rows(0..5);
    duplicate[3].frame = 2;
    let error = align(&duplicate, 5, 0).unwrap_err().to_string();
    assert!(error.contains("frame 2 follows frame 2"), "{error}");
    assert!(align(&[], 5, 0).is_err());
}

#[test]
fn coordinate_systems_must_agree() {
    use Coordinates::{Decoded, Stream};
    // 0..stream and leading..stream are both stream coordinates, also with a shot list.
    check_coordinates(Stream, Stream, true, 2).unwrap();
    check_coordinates(Decoded, Decoded, false, 2).unwrap();
    for (reference, candidate) in [(Stream, Decoded), (Decoded, Stream)] {
        let error = check_coordinates(reference, candidate, false, 2)
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot both describe"), "{error}");
    }
    let error = check_coordinates(Decoded, Decoded, true, 2)
        .unwrap_err()
        .to_string();
    assert!(error.contains("--per-shot cannot"), "{error}");
    // Without leading pictures the systems are the same.
    check_coordinates(Stream, Decoded, true, 0).unwrap();
}

#[test]
fn shot_lists_are_shifted_by_the_leading_pictures() {
    // 10 measured frames after 3 leading pictures: stream frames 0..13.
    assert_eq!(
        parse_shots("0\n2\n5\n9\n13\n", 10, 3).unwrap(),
        vec![0..2, 2..6, 6..10],
        "the shot 0..2 lies in the leading pictures, the sentinel 13 is dropped"
    );
    // A shot starting right after the leading pictures.
    assert_eq!(parse_shots("0\n3\n", 10, 3).unwrap(), vec![0..10]);
    assert_eq!(parse_shots("0\n4\n", 10, 3).unwrap(), vec![0..1, 1..10]);
    assert_eq!(parse_shots("0\n4\n", 10, 0).unwrap(), vec![0..4, 4..10]);
    for (text, message) in [
        ("", "no shot starts"),
        ("1\n4\n", "must start at frame 0"),
        ("0\n4\n4\n", "must increase"),
        ("0\n14\n", "beyond the 13 stream frames"),
        ("0\nx\n", "line 2"),
    ] {
        let error = format!("{:#}", parse_shots(text, 10, 3).unwrap_err());
        assert!(error.contains(message), "{text:?}: {error}");
    }
}
