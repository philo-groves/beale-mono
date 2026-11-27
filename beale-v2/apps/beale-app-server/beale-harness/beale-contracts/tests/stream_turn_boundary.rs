use beale_contracts::v1::{SessionDetail, SessionStreamEvent, SessionStreamKind};
use prost::Message;
#[test]
fn committed_and_live_turn_boundaries_roundtrip() {
    let event = SessionStreamEvent {
        kind: SessionStreamKind::SwarmUpdated as i32,
        turn_number: 3,
        detail: Some(SessionDetail {
            committed_turn_number: 2,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        SessionStreamEvent::decode(event.encode_to_vec().as_slice()).unwrap(),
        event
    );
    assert!(beale_contracts::session_stream_frame(&event).is_some());
}
