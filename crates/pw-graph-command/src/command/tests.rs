use super::*;
use pw_graph_backend::InMemoryDriver;

#[test]
fn connect_undo_redo_round_trip() {
    let mut driver = InMemoryDriver::demo();
    let mut commands = CommandStack::new();
    commands
        .execute(
            Box::new(ConnectCommand::new(PortId(1), PortId(3))),
            &mut driver,
        )
        .unwrap();
    assert!(commands.can_undo());
    assert_eq!(commands.undo_history().len(), 1);
    assert!(commands.undo_history()[0].starts_with("Connect:"));
    assert!(commands.redo_history().is_empty());
    assert_eq!(driver.graph().links.len(), 1);
    commands.undo(&mut driver).unwrap();
    assert!(driver.graph().links.is_empty());
    assert!(commands.undo_history().is_empty());
    assert_eq!(commands.redo_history().len(), 1);
    commands.redo(&mut driver).unwrap();
    assert_eq!(driver.graph().links.len(), 1);
}

#[test]
fn connecting_an_existing_pair_is_a_noop_for_undo() {
    let mut driver = InMemoryDriver::demo();
    let mut commands = CommandStack::new();
    commands
        .execute(
            Box::new(ConnectCommand::new(PortId(1), PortId(3))),
            &mut driver,
        )
        .unwrap();
    commands
        .execute(
            Box::new(ConnectCommand::new(PortId(1), PortId(3))),
            &mut driver,
        )
        .unwrap();
    assert_eq!(driver.graph().links.len(), 1);
    commands.undo(&mut driver).unwrap();
    assert_eq!(driver.graph().links.len(), 1);
    commands.undo(&mut driver).unwrap();
    assert!(driver.graph().links.is_empty());
}

#[test]
fn disconnect_undo_redo_round_trip() {
    let mut driver = InMemoryDriver::demo();
    let link = driver.connect(PortId(1), PortId(3)).unwrap();
    let mut commands = CommandStack::new();
    commands
        .execute(Box::new(DisconnectCommand::new(link.id)), &mut driver)
        .unwrap();
    assert!(driver.graph().links.is_empty());
    commands.undo(&mut driver).unwrap();
    assert_eq!(driver.graph().links.len(), 1);
    commands.redo(&mut driver).unwrap();
    assert!(driver.graph().links.is_empty());
}

#[test]
fn disconnect_all_undo_redo_round_trip() {
    let mut driver = InMemoryDriver::demo();
    driver.connect(PortId(1), PortId(3)).unwrap();
    driver.connect(PortId(2), PortId(4)).unwrap();
    let mut commands = CommandStack::new();

    commands
        .execute(Box::new(DisconnectAllCommand::new()), &mut driver)
        .unwrap();
    assert!(driver.graph().links.is_empty());
    commands.undo(&mut driver).unwrap();
    assert_eq!(driver.graph().links.len(), 2);
    commands.redo(&mut driver).unwrap();
    assert!(driver.graph().links.is_empty());
}

#[test]
fn disconnect_commands_leave_observed_links_in_place() {
    let mut driver = InMemoryDriver::demo();
    let link = driver.connect(PortId(1), PortId(3)).unwrap();
    driver.mark_link_observed(link.id);
    let mut commands = CommandStack::new();

    commands
        .execute(Box::new(DisconnectAllCommand::new()), &mut driver)
        .unwrap();
    assert!(driver.graph().link(link.id).is_some());
    commands.undo(&mut driver).unwrap();
    assert!(driver.graph().link(link.id).is_some());

    commands
        .execute(Box::new(DisconnectCommand::new(link.id)), &mut driver)
        .unwrap();
    assert!(driver.graph().link(link.id).is_some());
}

#[test]
fn disconnect_many_is_one_undoable_operation() {
    let mut driver = InMemoryDriver::demo();
    let first = driver.connect(PortId(1), PortId(3)).unwrap();
    let second = driver.connect(PortId(2), PortId(4)).unwrap();
    let mut commands = CommandStack::new();

    commands
        .execute(
            Box::new(DisconnectManyCommand::new(vec![first.id, second.id])),
            &mut driver,
        )
        .unwrap();
    assert!(driver.graph().links.is_empty());
    commands.undo(&mut driver).unwrap();
    assert_eq!(driver.graph().links.len(), 2);
    commands.redo(&mut driver).unwrap();
    assert!(driver.graph().links.is_empty());
}

#[test]
fn connect_many_is_one_undoable_operation() {
    let mut driver = InMemoryDriver::demo();
    let mut commands = CommandStack::new();
    commands
        .execute(
            Box::new(ConnectManyCommand::new(vec![
                (PortId(1), PortId(3)),
                (PortId(2), PortId(4)),
            ])),
            &mut driver,
        )
        .unwrap();
    assert_eq!(driver.graph().links.len(), 2);
    commands.undo(&mut driver).unwrap();
    assert!(driver.graph().links.is_empty());
}

#[test]
fn connect_many_undo_reconnects_links_after_a_later_disconnect_failure() {
    let mut driver = InMemoryDriver::demo();
    let mut commands = CommandStack::new();
    commands
        .execute(
            Box::new(ConnectManyCommand::new(vec![
                (PortId(1), PortId(3)),
                (PortId(2), PortId(4)),
            ])),
            &mut driver,
        )
        .unwrap();
    driver.fail_disconnect_of_pair(PortId(1), PortId(3));

    let error = commands
        .undo(&mut driver)
        .expect_err("the second removal fails");
    assert!(matches!(error, CommandError::Backend(_)), "{error:?}");
    assert!(is_connected(&driver, PortId(1), PortId(3)));
    assert!(is_connected(&driver, PortId(2), PortId(4)));
    assert!(commands.can_undo(), "a failed undo remains retryable");
}

#[test]
fn connect_many_undo_reports_a_failed_reconnect_without_clearing_undo_state() {
    let mut driver = InMemoryDriver::demo();
    let mut commands = CommandStack::new();
    commands
        .execute(
            Box::new(ConnectManyCommand::new(vec![
                (PortId(1), PortId(3)),
                (PortId(2), PortId(4)),
            ])),
            &mut driver,
        )
        .unwrap();
    driver.fail_disconnect_of_pair(PortId(1), PortId(3));
    driver.fail_connect_of(PortId(2), PortId(4));

    let error = commands
        .undo(&mut driver)
        .expect_err("reconnect also fails");
    match error {
        CommandError::PartiallyApplied {
            operation,
            stranded,
            ..
        } => {
            assert_eq!(operation, "Connect group undo");
            assert_eq!(stranded, 1);
        }
        other => panic!("expected partial undo, got {other:?}"),
    }
    assert!(is_connected(&driver, PortId(1), PortId(3)));
    assert!(!is_connected(&driver, PortId(2), PortId(4)));
    assert!(commands.can_undo());
}

#[test]
fn a_failed_group_disconnect_leaves_the_graph_untouched() {
    // The regression this guards: the first links were removed, the error
    // propagated, and `CommandStack::execute` then refused to record the
    // command — so the user lost connections with no undo available.
    let mut driver = InMemoryDriver::demo();
    let first = driver.connect(PortId(1), PortId(3)).unwrap();
    let second = driver.connect(PortId(2), PortId(4)).unwrap();
    driver.fail_disconnect_of(second.id);
    let mut commands = CommandStack::new();

    let error = commands
        .execute(
            Box::new(DisconnectManyCommand::new(vec![first.id, second.id])),
            &mut driver,
        )
        .expect_err("the group disconnect must fail");
    assert!(matches!(error, CommandError::Backend(_)));
    assert_eq!(
        driver.graph().links.len(),
        2,
        "a failed group disconnect must roll back the links it removed"
    );
    assert!(!commands.can_undo());
}

#[test]
fn reroute_undo_keeps_a_connection_it_did_not_create() {
    // Rerouting onto a pair that already exists must not give undo licence
    // to delete somebody else's connection.
    let mut driver = InMemoryDriver::demo();
    let moving = driver.connect(PortId(1), PortId(3)).unwrap();
    let existing = driver.connect(PortId(2), PortId(4)).unwrap();
    let mut commands = CommandStack::new();

    // Drag the source end of `moving` onto port 2, which makes it the same
    // pair as `existing`.
    commands
        .execute(
            Box::new(RerouteLinkCommand::new(moving.id, PortId(2))),
            &mut driver,
        )
        .unwrap();
    assert!(driver.graph().link(existing.id).is_some());

    commands.undo(&mut driver).unwrap();
    assert!(
        driver
            .graph()
            .find_link_by_keys(
                &driver.graph().port_key(PortId(2)).unwrap(),
                &driver.graph().port_key(PortId(4)).unwrap()
            )
            .is_some(),
        "undo must not delete a pre-existing connection it did not create"
    );
}

#[test]
fn reroute_redo_uses_stable_endpoints_after_restore_renumbers_link() {
    let mut driver = InMemoryDriver::demo();
    let old = driver.connect(PortId(1), PortId(3)).unwrap();
    let mut commands = CommandStack::new();

    commands
        .execute(
            Box::new(RerouteLinkCommand::new(old.id, PortId(2))),
            &mut driver,
        )
        .unwrap();
    commands.undo(&mut driver).unwrap();

    // InMemoryDriver allocates a fresh ID when the original route is
    // restored. Redo must therefore use the stable endpoint keys.
    let restored = driver
        .graph()
        .find_link_by_keys(
            &driver.graph().port_key(PortId(1)).unwrap(),
            &driver.graph().port_key(PortId(3)).unwrap(),
        )
        .unwrap();
    assert_ne!(restored.id, old.id);

    commands.redo(&mut driver).unwrap();
    assert!(!is_connected(&driver, PortId(1), PortId(3)));
    assert!(is_connected(&driver, PortId(2), PortId(3)));
}

#[test]
fn a_failed_reroute_restores_the_original_route() {
    let mut driver = InMemoryDriver::demo();
    let link = driver.connect(PortId(1), PortId(3)).unwrap();
    driver.fail_connect_of(PortId(2), PortId(3));
    let mut commands = CommandStack::new();

    assert!(commands
        .execute(
            Box::new(RerouteLinkCommand::new(link.id, PortId(2))),
            &mut driver
        )
        .is_err());
    assert_eq!(
        driver.graph().links.len(),
        1,
        "the original route must come back when the new one is refused"
    );
}

#[test]
fn move_nodes_undo_redo_round_trip() {
    let mut driver = InMemoryDriver::demo();
    let before = driver.graph().node(NodeId(1)).unwrap().position;
    let after = [300.0, 200.0];
    let mut commands = CommandStack::new();
    commands
        .execute(
            Box::new(MoveNodesCommand::new(
                vec![(NodeId(1), before)],
                vec![(NodeId(1), after)],
            )),
            &mut driver,
        )
        .unwrap();
    assert_eq!(driver.graph().node(NodeId(1)).unwrap().position, after);
    commands.undo(&mut driver).unwrap();
    assert_eq!(driver.graph().node(NodeId(1)).unwrap().position, before);
    commands.redo(&mut driver).unwrap();
    assert_eq!(driver.graph().node(NodeId(1)).unwrap().position, after);
}

#[test]
fn a_later_node_move_failure_restores_earlier_nodes() {
    let mut driver = InMemoryDriver::demo();
    let before_a = driver.graph().node(NodeId(1)).unwrap().position;
    let before_b = driver.graph().node(NodeId(2)).unwrap().position;
    let after_a = [300.0, 200.0];
    let after_b = [700.0, 200.0];
    driver.fail_position_at(NodeId(2), after_b);

    let mut command = MoveNodesCommand::new(
        vec![(NodeId(1), before_a), (NodeId(2), before_b)],
        vec![(NodeId(1), after_a), (NodeId(2), after_b)],
    );
    let error = command
        .execute(&mut driver)
        .expect_err("the later node move fails");
    assert!(matches!(error, CommandError::Backend(_)), "{error:?}");
    assert_eq!(driver.graph().node(NodeId(1)).unwrap().position, before_a);
    assert_eq!(driver.graph().node(NodeId(2)).unwrap().position, before_b);
}

#[test]
fn node_move_reports_when_restoring_an_earlier_node_fails() {
    let mut driver = InMemoryDriver::demo();
    let before_a = driver.graph().node(NodeId(1)).unwrap().position;
    let before_b = driver.graph().node(NodeId(2)).unwrap().position;
    let after_a = [300.0, 200.0];
    let after_b = [700.0, 200.0];
    driver.fail_position_at(NodeId(2), after_b);
    driver.fail_position_at(NodeId(1), before_a);

    let mut command = MoveNodesCommand::new(
        vec![(NodeId(1), before_a), (NodeId(2), before_b)],
        vec![(NodeId(1), after_a), (NodeId(2), after_b)],
    );
    let error = command
        .execute(&mut driver)
        .expect_err("the failed restore must be explicit");
    match error {
        CommandError::PartiallyApplied {
            operation,
            stranded,
            ..
        } => {
            assert_eq!(operation, "Move nodes");
            assert_eq!(stranded, 1);
        }
        other => panic!("expected partial node move, got {other:?}"),
    }
    assert_eq!(driver.graph().node(NodeId(1)).unwrap().position, after_a);
}

/// The demo graph's two source ports and two sink ports, as stable keys.
fn key(driver: &InMemoryDriver, port: PortId) -> PortKey {
    driver.graph().port_key(port).expect("demo port exists")
}

fn is_connected(driver: &InMemoryDriver, output: PortId, input: PortId) -> bool {
    driver
        .graph()
        .find_link_by_keys(&key(driver, output), &key(driver, input))
        .is_some()
}

#[test]
fn a_failed_group_connect_rolls_every_created_link_back() {
    let mut driver = InMemoryDriver::demo();
    driver.fail_connect_of(PortId(2), PortId(4));
    let mut commands = CommandStack::new();

    let error = commands
        .execute(
            Box::new(ConnectManyCommand::new(vec![
                (PortId(1), PortId(3)),
                (PortId(2), PortId(4)),
            ])),
            &mut driver,
        )
        .expect_err("the second pair is refused");

    // Rollback worked, so the caller sees the real cause, not a
    // partially-applied report.
    assert!(matches!(error, CommandError::Backend(_)), "{error:?}");
    assert!(
        driver.graph().links.is_empty(),
        "a created link was left behind"
    );
    assert!(
        !commands.can_undo(),
        "a failed command must not reach the undo stack"
    );
}

#[test]
fn a_group_connect_failing_on_a_later_pair_still_rolls_everything_back() {
    let mut driver = InMemoryDriver::demo();
    driver.fail_connect_of(PortId(2), PortId(3));
    let mut commands = CommandStack::new();

    let error = commands
        .execute(
            Box::new(ConnectManyCommand::new(vec![
                (PortId(1), PortId(3)),
                (PortId(1), PortId(4)),
                (PortId(2), PortId(3)),
            ])),
            &mut driver,
        )
        .expect_err("the third pair is refused");

    assert!(matches!(error, CommandError::Backend(_)), "{error:?}");
    assert!(driver.graph().links.is_empty());
}

#[test]
fn a_group_connect_whose_rollback_fails_reports_the_stranded_links() {
    // The regression: the rollback's own errors were discarded, so a
    // command that had left links behind reported a plain backend
    // failure — and, never having completed, was not on the undo stack
    // either, so nothing offered to clean them up.
    let mut driver = InMemoryDriver::demo();
    driver.fail_connect_of(PortId(2), PortId(4));
    driver.fail_disconnect_of_pair(PortId(1), PortId(3));
    let mut commands = CommandStack::new();

    let error = commands
        .execute(
            Box::new(ConnectManyCommand::new(vec![
                (PortId(1), PortId(3)),
                (PortId(2), PortId(4)),
            ])),
            &mut driver,
        )
        .expect_err("the second pair is refused");

    match error {
        CommandError::PartiallyApplied {
            operation,
            stranded,
            ..
        } => {
            assert_eq!(operation, "Connect group");
            assert_eq!(stranded, 1);
        }
        other => panic!("expected a partially-applied report, got {other:?}"),
    }
    assert!(
        is_connected(&driver, PortId(1), PortId(3)),
        "the test wired this link to be unremovable"
    );
    assert!(!is_connected(&driver, PortId(2), PortId(4)));
}

#[test]
fn a_group_connect_rollback_leaves_pre_existing_links_alone() {
    // `connect_by_key_if_missing` returns `None` for a pair that already
    // exists, so it never enters the created set — and the rollback must
    // not disconnect something this command did not create.
    let mut driver = InMemoryDriver::demo();
    driver.connect(PortId(1), PortId(3)).unwrap();
    driver.fail_connect_of(PortId(2), PortId(4));
    let mut commands = CommandStack::new();

    assert!(commands
        .execute(
            Box::new(ConnectManyCommand::new(vec![
                (PortId(1), PortId(3)),
                (PortId(1), PortId(4)),
                (PortId(2), PortId(4)),
            ])),
            &mut driver,
        )
        .is_err());

    assert!(
        is_connected(&driver, PortId(1), PortId(3)),
        "rollback removed a link the command never created"
    );
    assert!(!is_connected(&driver, PortId(1), PortId(4)));
    assert_eq!(driver.graph().links.len(), 1);
}

#[test]
fn connect_group_rollback_only_clears_its_own_suppression_state() {
    let mut driver = InMemoryDriver::demo();
    let unrelated_output = key(&driver, PortId(1));
    let unrelated_input = key(&driver, PortId(4));
    driver.mark_connection_suppressed(unrelated_output.clone(), unrelated_input.clone());
    driver.fail_connect_of(PortId(2), PortId(4));
    let mut commands = CommandStack::new();

    assert!(commands
        .execute(
            Box::new(ConnectManyCommand::new(vec![
                (PortId(1), PortId(3)),
                (PortId(2), PortId(4)),
            ])),
            &mut driver,
        )
        .is_err());

    // The successful first pair was disconnected by rollback and its
    // explicit allow step cleared only that pair. A suppression belonging
    // to an unrelated route must survive the transaction.
    assert!(!driver.is_connection_suppressed(&key(&driver, PortId(1)), &key(&driver, PortId(3))));
    assert!(driver.is_connection_suppressed(&unrelated_output, &unrelated_input));
}

#[test]
fn connect_many_noop_does_not_unsuppress_a_preexisting_pair() {
    let mut driver = InMemoryDriver::demo();
    driver.connect(PortId(1), PortId(3)).unwrap();
    let output = key(&driver, PortId(1));
    let input = key(&driver, PortId(3));
    driver.mark_connection_suppressed(output.clone(), input.clone());

    let mut commands = CommandStack::new();
    commands
        .execute(
            Box::new(ConnectManyCommand::new(vec![(PortId(1), PortId(3))])),
            &mut driver,
        )
        .unwrap();

    assert!(driver.is_connection_suppressed(&output, &input));
    assert!(is_connected(&driver, PortId(1), PortId(3)));
}

#[test]
fn a_reroute_undo_that_cannot_restore_either_route_says_so() {
    // Undo removes the rerouted link to make room, then fails to restore
    // the original, then fails to put the rerouted one back. The graph is
    // left with neither connection; reporting only the first error — as
    // this used to — described that as an ordinary backend failure.
    let mut driver = InMemoryDriver::demo();
    let link = driver.connect(PortId(1), PortId(3)).unwrap();
    let mut commands = CommandStack::new();
    commands
        .execute(
            Box::new(RerouteLinkCommand::new(link.id, PortId(2))),
            &mut driver,
        )
        .expect("the reroute itself succeeds");
    assert!(is_connected(&driver, PortId(2), PortId(3)));

    driver.fail_connect_of(PortId(1), PortId(3));
    driver.fail_connect_of(PortId(2), PortId(3));
    let error = commands
        .undo(&mut driver)
        .expect_err("undo cannot complete");

    match error {
        CommandError::PartiallyApplied {
            operation,
            cause,
            stranded,
        } => {
            assert_eq!(operation, "Reroute");
            assert_eq!(stranded, 2);
            assert!(
                cause.contains("could not be put") && cause.contains("original"),
                "the report must name both failures: {cause}"
            );
        }
        other => panic!("expected a partially-applied report, got {other:?}"),
    }
    assert!(driver.graph().links.is_empty(), "neither route survived");
}

#[test]
fn a_reroute_undo_that_can_put_the_new_route_back_reports_a_plain_error() {
    // Only the first level failed, so the graph is exactly where it was
    // before the undo started. That is an ordinary backend error, not a
    // partial application.
    let mut driver = InMemoryDriver::demo();
    let link = driver.connect(PortId(1), PortId(3)).unwrap();
    let mut commands = CommandStack::new();
    commands
        .execute(
            Box::new(RerouteLinkCommand::new(link.id, PortId(2))),
            &mut driver,
        )
        .expect("the reroute itself succeeds");

    driver.fail_connect_of(PortId(1), PortId(3));
    let error = commands
        .undo(&mut driver)
        .expect_err("undo cannot complete");

    assert!(matches!(error, CommandError::Backend(_)), "{error:?}");
    assert!(
        is_connected(&driver, PortId(2), PortId(3)),
        "the rerouted link must be back"
    );
    assert!(!is_connected(&driver, PortId(1), PortId(3)));
}
