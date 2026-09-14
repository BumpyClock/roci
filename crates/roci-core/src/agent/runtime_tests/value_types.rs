use super::*;

#[test]
fn queue_drain_mode_all_preserves_order_and_empties_queue() {
    let mut queue = vec![ModelMessage::user("one"), ModelMessage::user("two")];
    let drained = drain_queue(&mut queue, QueueDrainMode::All);
    assert_eq!(
        drained.iter().map(ModelMessage::text).collect::<Vec<_>>(),
        ["one", "two"]
    );
    assert!(queue.is_empty());
    assert!(drain_queue(&mut queue, QueueDrainMode::All).is_empty());
}

#[test]
fn queue_drain_mode_one_at_a_time_preserves_order_until_empty() {
    let mut queue = vec![
        ModelMessage::user("one"),
        ModelMessage::user("two"),
        ModelMessage::user("three"),
    ];
    for (index, expected) in ["one", "two", "three"].into_iter().enumerate() {
        let drained = drain_queue(&mut queue, QueueDrainMode::OneAtATime);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].text(), expected);
        assert_eq!(queue.len(), 2 - index);
    }
    assert!(drain_queue(&mut queue, QueueDrainMode::OneAtATime).is_empty());
}
