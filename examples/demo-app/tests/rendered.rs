//! The results of the collection in `demo_app::plan`.

use demo_app::handlers::handlers;
use demo_app::mixed::mixed;
use demo_app::orders::orders;
use demo_app::users::users;
use demo_app::{plan, sql};
use demo_upstream::accounts;
use plenary::Collection;

#[test]
fn plain_query_renders_with_positional_params() {
    assert_eq!(sql::<users::OrgUsers>(), "SELECT id, email FROM users WHERE org_id = $1");
}

#[test]
fn select_star_is_expanded_to_the_table_columns() {
    assert_eq!(sql::<users::AllUsers>(), "SELECT id, email, org_id FROM users");
    assert_eq!(
        sql::<orders::RecentOrders>(),
        "SELECT id, user_id, total_cents, placed_at FROM orders WHERE placed_at > $1"
    );
}

#[test]
fn query_in_orders_rs_can_see_table_from_users_rs() {
    assert_eq!(sql::<orders::OrderOwnerEmail>(), "SELECT email FROM users WHERE id = $1");
}

#[test]
fn query_in_a_nested_module_is_rendered() {
    assert_eq!(sql::<users::admin::EmailById>(), "SELECT email FROM users WHERE id = $1");
}

#[test]
fn query_in_this_crate_can_see_table_from_upstream_crate() {
    // `accounts` is defined in demo-upstream; `SELECT *` expands against its
    // column list, which only the carrier could have told us.
    assert_eq!(
        sql::<orders::AccountForOrder>(),
        "SELECT id, owner_email, balance_cents FROM accounts WHERE id = $1"
    );
}

#[test]
fn upstream_query_is_rendered_by_the_downstream_collection() {
    // The upstream crate's own query gets a `Query<__Collection>` impl from
    // OUR collection, with our local marker (orphan-rule workaround).
    assert_eq!(
        sql::<accounts::AccountByEmail>(),
        "SELECT id, balance_cents FROM accounts WHERE owner_email = $1"
    );
    // ... and also from its own collection, with its own marker.
    assert_eq!(
        <accounts::AccountByEmail as stq_lite::Query<demo_upstream::plan::__Collection>>::SQL,
        "SELECT id, balance_cents FROM accounts WHERE owner_email = $1"
    );
}

#[test]
fn inline_sql_inside_a_function_body() {
    assert_eq!(mixed::count_users_sql(""), "SELECT id, email, org_id FROM users WHERE org_id = $1");
    // Upstream's inline query was resolved by upstream's own collection.
    assert_eq!(accounts::richest_sql(), "SELECT id FROM accounts WHERE balance_cents > $1");
}

#[test]
fn dispatch_finds_handlers_from_every_source() {
    assert_eq!(plan::dispatch("greet", "bob").as_deref(), Some("hello, bob"));
    assert_eq!(plan::dispatch("shout", "bob").as_deref(), Some("BOB"));
    assert_eq!(plan::dispatch("upstream_echo", "bob").as_deref(), Some("upstream says bob"));
    assert_eq!(plan::dispatch("count_users_sql", "").as_deref(), Some("SELECT id, email, org_id FROM users WHERE org_id = $1"));
    assert_eq!(plan::dispatch("nope", "bob"), None);
}

#[test]
fn lookups_inside_function_bodies() {
    assert_eq!(handlers::shout_twice("bob"), "BOB");
    assert_eq!(mixed::greet_via_lookup("ann"), "hello, ann");
    assert_eq!(mixed::deeper::upstream("x"), "upstream says x");
}

#[test]
fn handlers_table_is_in_canonical_order() {
    let names: Vec<&str> = plan::HANDLERS.iter().map(|(n, _)| *n).collect();
    let mut expected = vec!["greet", "shout"];
    if cfg!(feature = "extra") {
        expected.push("extra");
    }
    expected.extend(["count_users_sql", "upstream_echo"]);
    assert_eq!(names, expected);
}

#[test]
fn extra_handler_follows_the_feature() {
    // G5: the record is in the state if and only if its cfg holds.
    let present = plan::dispatch("extra", "x").is_some();
    assert_eq!(present, cfg!(feature = "extra"));
    if present {
        assert_eq!(plan::dispatch("extra", "x").as_deref(), Some("extra: x"));
    }
}

#[test]
fn summary_counts_everything_the_collection_saw() {
    let handlers = if cfg!(feature = "extra") { 5 } else { 4 };
    // tables: users, orders, accounts. queries: OrgUsers, AllUsers, EmailById,
    // RecentOrders, OrderOwnerEmail, AccountForOrder, AccountByEmail.
    // inline: mixed::count_users_sql, accounts::richest_sql.
    assert_eq!(plan::SUMMARY, format!("tables: 3, queries: 7, inline queries: 2, handlers: {handlers}"));
}

#[test]
fn collection_lists_its_sources_in_order() {
    assert_eq!(
        <plan::__Collection as Collection>::SOURCES,
        &[
            "crate::users::users",
            "crate::orders::orders",
            "crate::handlers::handlers",
            "crate::mixed::mixed",
            "demo_upstream::accounts",
            "demo_upstream::upstream_handlers",
        ]
    );
}

#[test]
fn library_code_uses_the_generated_items() {
    use demo_app::consumers;
    assert_eq!(consumers::greet_all(&["a", "b"]), vec!["hello, a", "hello, b"]);
    assert_eq!(consumers::user_queries(), ["SELECT id, email FROM users WHERE org_id = $1", "SELECT id, email, org_id FROM users"]);
    assert_eq!(consumers::summary(), plan::SUMMARY);
    assert_eq!(consumers::sources().len(), 6);
    assert_eq!(consumers::shout("x"), "X");
    assert!(consumers::handler_names().contains(&"greet"));
}

// ───────────────────────────── stages (SPEC section 10) ─────────────────────────────

mod stages {
    use demo_app::lookups::lookups;
    use demo_app::staged::staged;
    use demo_app::{plan, plan2, plan3, sql2};
    use demo_upstream::accounts;
    use plenary::Collection;

    #[test]
    fn a_query_resolves_against_a_table_known_only_through_a_stage() {
        // `plan2` lists no raw upstream source; `accounts` reached it as an
        // exported fact of `demo_upstream::plan`.
        assert_eq!(sql2::<staged::RichOwners>(), "SELECT owner_email FROM accounts WHERE balance_cents > $1");
        // `SELECT *` expanded against the imported column list.
        assert_eq!(sql2::<staged::AccountById>(), "SELECT id, owner_email, balance_cents FROM accounts WHERE id = $1");
        assert_eq!(
            <plan2::__Collection as Collection>::SOURCES,
            &["demo_upstream::plan", "crate::staged::staged"]
        );
    }

    #[test]
    fn imported_queries_get_this_collections_impls_too() {
        // The upstream query came through the stage (its path rebased from
        // `crate::accounts::AccountByEmail` to `demo_upstream::accounts::..`).
        assert_eq!(sql2::<accounts::AccountByEmail>(), "SELECT id, balance_cents FROM accounts WHERE owner_email = $1");
    }

    #[test]
    fn imported_handlers_and_the_generated_record_are_dispatchable() {
        // `upstream_echo` arrived in the exported `Index`; `generated_echo` is
        // the handler demo-upstream's registry emitted, found as a record of
        // the stage (one round per collection: it is not in upstream's own
        // HANDLERS).
        let names: Vec<&str> = plan2::HANDLERS.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["upstream_echo", "generated_echo"]);
        assert_eq!(plan2::dispatch("upstream_echo", "x").as_deref(), Some("upstream says x"));
        assert_eq!(plan2::dispatch("generated_echo", "x").as_deref(), Some("echo from a registry of 1 handlers: x"));
        assert_eq!(demo_upstream::plan::dispatch("generated_echo", "x"), None, "not in its own round");
        assert_eq!(plan::dispatch("generated_echo", "x"), None, "plan lists the raw sources, not the stage");
    }

    #[test]
    fn stats_count_what_the_stage_provided() {
        // tables: accounts (imported). queries: AccountByEmail (imported),
        // RichOwners, AccountById. inline: richest_sql (imported, no impl
        // here). handlers: upstream_echo (imported), generated_echo (record).
        assert_eq!(plan2::SUMMARY, "tables: 1, queries: 3, inline queries: 1, handlers: 2");
    }

    #[test]
    fn a_same_crate_round_sees_the_generated_handler() {
        // `plan3` lists `crate::plan` as a stage: plan's whole index plus the
        // handler plan generated, and a lookup in lookups.rs resolves to it.
        let n = if cfg!(feature = "extra") { 5 } else { 4 };
        assert_eq!(lookups::generated("hi"), format!("echo from a registry of {n} handlers: hi"));
        assert_eq!(lookups::upstream_via_stage("hi"), "upstream says hi");
        let names: Vec<&str> = plan3::HANDLERS.iter().map(|(n, _)| *n).collect();
        let mut expected = vec!["greet", "shout"];
        if cfg!(feature = "extra") {
            expected.push("extra");
        }
        expected.extend(["count_users_sql", "upstream_echo", "generated_echo"]);
        assert_eq!(names, expected);
        assert_eq!(plan3::dispatch("generated_echo", "x").as_deref(), Some(format!("echo from a registry of {n} handlers: x").as_str()));
        assert_eq!(<plan3::__Collection as Collection>::SOURCES, &["crate::plan", "crate::lookups::lookups"]);
    }

    #[test]
    fn imported_facts_from_a_local_stage_keep_crate_paths() {
        // plan3 re-renders plan's queries with its own marker: the paths in
        // the imported fact stayed `crate::..` because the stage is local.
        use demo_app::users::users::OrgUsers;
        assert_eq!(<OrgUsers as stq_lite::Query<plan3::__Collection>>::SQL, "SELECT id, email FROM users WHERE org_id = $1");
        assert_eq!(plan3::SUMMARY, plan::SUMMARY.replace(&format!("handlers: {}", plan::HANDLERS.len()), &format!("handlers: {}", plan::HANDLERS.len() + 1)));
    }
}

// ───────────────────────────── closed sets (SPEC section 11) ─────────────────────────────

mod closed {
    use demo_app::consumers;
    use demo_app::{plan, plan2, plan3};

    #[test]
    fn all_has_one_variant_per_handler_in_canonical_order() {
        let names: Vec<&str> = plan::Handler::ALL.iter().map(|h| h.name()).collect();
        let mut expected = vec!["greet", "shout"];
        if cfg!(feature = "extra") {
            expected.push("extra");
        }
        expected.extend(["count_users_sql", "upstream_echo"]);
        assert_eq!(names, expected);
        assert_eq!(plan::Handler::ALL.len(), plan::HANDLERS.len());
        assert_eq!(plan::Handler::COUNT, plan::HANDLERS.len());
    }

    #[test]
    fn names_ids_and_variants_round_trip() {
        for (i, h) in plan::Handler::ALL.iter().enumerate() {
            assert_eq!(plan::Handler::from_name(h.name()), Some(*h));
            assert_eq!(h.id() as usize, i, "dense discriminants in canonical order");
            assert_eq!(plan::Handler::try_from(h.id()), Ok(*h));
            assert_eq!(*h as u16, h.id());
        }
        assert_eq!(plan::Handler::from_name("nope"), None);
        assert_eq!(plan::Handler::try_from(999), Err(999));
        assert_eq!(plan::Handler::Greet.name(), "greet");
        assert_eq!(plan::Handler::CountUsersSql.name(), "count_users_sql");
    }

    #[test]
    fn call_dispatches_through_the_checked_functions() {
        assert_eq!(plan::Handler::Greet.call("bob"), "hello, bob");
        assert_eq!(plan::Handler::Shout.call("bob"), "BOB");
        assert_eq!(plan::Handler::UpstreamEcho.call("x"), "upstream says x");
        assert_eq!(plan::Handler::Greet.function()("ann"), "hello, ann");
        // `dispatch` is `from_name(..).map(call)`.
        assert_eq!(plan::dispatch("shout", "x").as_deref(), Some("X"));
    }

    #[test]
    fn an_exhaustive_match_covers_every_handler() {
        let described: Vec<&str> = plan::Handler::ALL.iter().map(|h| consumers::describe(*h)).collect();
        assert_eq!(described.len(), plan::Handler::COUNT);
        assert_eq!(consumers::describe(plan::Handler::Greet), "greets the input");
    }

    #[test]
    fn variant_resolves_a_name_to_the_enum_at_build_time() {
        assert_eq!(consumers::shout_variant(), plan::Handler::Shout);
        assert_eq!(consumers::shout_variant().call("x"), "X");
    }

    #[test]
    fn the_enum_at_a_stage_consumer_includes_imported_and_generated_handlers() {
        // plan2: upstream's index came through the stage, generated_echo as a record.
        let names: Vec<&str> = plan2::Handler::ALL.iter().map(|h| h.name()).collect();
        assert_eq!(names, ["upstream_echo", "generated_echo"]);
        assert_eq!(plan2::Handler::GeneratedEcho.call("x"), "echo from a registry of 1 handlers: x");
        assert_eq!(plan2::Handler::UpstreamEcho.id(), 0);
        // plan3: plan's whole index plus plan's generated handler.
        assert_eq!(plan3::Handler::ALL.len(), plan::Handler::ALL.len() + 1);
        assert_eq!(plan3::Handler::ALL.last().map(|h| h.name()), Some("generated_echo"));
        assert!(plan3::Handler::from_name("greet").is_some());
    }
}
