use std::path::Path;

use buizz_core::protocol::{BotMessage, SpineMessage, decode, encode};

#[test]
fn every_golden_frame_round_trips_through_core_types() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/protocol");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("fixtures/protocol exists") {
        let path = entry.expect("readable entry").path();
        if path
            .extension()
            .is_none_or(|extension| extension != "msgpack")
        {
            continue;
        }
        let frame = std::fs::read(&path).expect("readable frame");
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default();
        let reencoded = if ["hello", "query", "ack"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            encode(&decode::<BotMessage>(&frame).unwrap_or_else(|error| panic!("{name}: {error}")))
        } else {
            encode(
                &decode::<SpineMessage>(&frame).unwrap_or_else(|error| panic!("{name}: {error}")),
            )
        };
        assert_eq!(
            reencoded.expect("re-encodes"),
            frame,
            "{name} changed on round trip"
        );
        checked += 1;
    }
    assert_eq!(checked, 22);
}
