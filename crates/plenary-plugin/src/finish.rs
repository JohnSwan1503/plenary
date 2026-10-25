//! `__finish`: decode every carrier, build the `State` and the imported
//! facts, run the global handlers in dependency order, run one *round* over
//! the emitted items, and assemble the collection's output including its
//! stage carrier (SPEC section 10).

use crate::carrier::{decode_source, parse_finish_input, DecodedSource, EncodedFact, SourceDesc};
use crate::collect::{carrier_items, collect_round, plugin_versions};
use crate::diag::{span_key, Diagnostic, Diagnostics};
use crate::facts::rebase_paths;
use crate::global::{collection_marker, order_handlers, registered, run_handler, ExportableInfo, GlobalCx, HandlerOutput};
use crate::plugin::{major_of, Bundle, FactStore};
use crate::record::{Record, SourceInfo, State};
use crate::util::{digest, path_string};
use proc_macro2::{Span, TokenStream};
use std::any::{Any, TypeId};

/// Run the collection on the tokens `expand!` built.
pub fn finish_impl(bundle: &Bundle, input: TokenStream) -> TokenStream {
    let raw_input = input.to_string();
    let input = match parse_finish_input(input) {
        Ok(i) => i,
        Err(e) => return e.to_compile_errors(),
    };
    let plenary = input.plenary.clone();
    let site = input.site;
    let export = input.export;
    let mut diagnostics = Diagnostics::new();

    // Decode sources, checking versions (R10) and duplicates.
    let mut sources: Vec<SourceInfo> = Vec::new();
    let mut records: Vec<Record> = Vec::new();
    let mut staged: Vec<(SourceInfo, Vec<EncodedFact>)> = Vec::new();
    let mut listed: Vec<String> = Vec::new();
    for (index, (path, body, span)) in input.sources.into_iter().enumerate() {
        let text = path_string(&path);
        if listed.contains(&text) {
            diagnostics.error(
                path.first().map(|p| p.span()).unwrap_or(span),
                format!("source `{text}` is listed twice in this `expand!`"),
            );
            // Keep the index space consistent: a placeholder failed source.
            sources.push(SourceInfo { index, path, export: false, failed: true, stage: false, plugins: Vec::new() });
            continue;
        }
        listed.push(text.clone());
        match decode_source(index, path.clone(), body) {
            Ok(DecodedSource { info, records: recs, facts, .. }) => {
                let keep = check_plugins(bundle, &info, site, &mut diagnostics);
                sources.push(info.clone());
                if keep {
                    records.extend(recs.into_iter().filter(|r| known_record(bundle, r)));
                    if info.stage && !facts.is_empty() {
                        staged.push((info, facts));
                    }
                }
            }
            Err(e) => {
                diagnostics.extend_from_error(e);
                sources.push(SourceInfo { index, path, export: false, failed: true, stage: false, plugins: Vec::new() });
            }
        }
    }

    // Imported facts: one `Imported<F>` per registered exportable type,
    // filled in canonical order (stage position, then export order).
    let mut imported: Vec<(TypeId, TypeId, Box<dyn Any>)> =
        bundle.exportables().map(|(_, e)| (e.id, e.imported_id, e.new_imported())).collect();
    for (info, facts) in &staged {
        for fact in facts {
            let Some((plugin, def)) = bundle.exportable_named(&fact.namespace, &fact.name) else {
                // A fact type this bundle doesn't know (R9 spirit): ignored.
                continue;
            };
            let at = info.path.first().map(|p| p.span()).unwrap_or(site);
            if major_of(&fact.version) != plugin.major_version() {
                diagnostics.error(
                    at,
                    format!(
                        "stage `{}` exported `{}::{}` with plugin `{}` v{}, but this bundle has `{}` v{} (major versions differ)",
                        info.display_path(),
                        fact.namespace,
                        fact.name,
                        plugin.name(),
                        fact.version,
                        plugin.name(),
                        plugin.version()
                    ),
                );
                continue;
            }
            // Paths in the fact meant something at the stage's own site (R3).
            let tokens = rebase_paths(fact.tokens.clone(), &info.path);
            match def.decode(tokens) {
                Ok(value) => {
                    let slot = imported.iter_mut().find(|(id, _, _)| *id == def.id).expect("registered");
                    def.push_imported(slot.2.as_mut(), info.clone(), value);
                }
                Err(e) => {
                    for d in e.into_diagnostics() {
                        diagnostics.error(
                            at,
                            format!(
                                "could not decode `{}::{}` exported by stage `{}`: {}",
                                fact.namespace,
                                fact.name,
                                info.display_path(),
                                d.rendered_message()
                            ),
                        );
                    }
                }
            }
        }
    }

    let source_names: Vec<String> = sources.iter().map(|s| s.display_path()).collect();
    let state = State::new(sources, records);

    let mut out = TokenStream::new();
    out.extend(collection_marker(&plenary, &source_names));

    // Order and run the global handlers (R6). `State` and every `Imported<F>`
    // are built in.
    let handlers = registered(bundle);
    let builtin: Vec<crate::FactId> =
        bundle.exportables().map(|(_, e)| crate::FactId { id: e.imported_id, name: e.type_name }).collect();
    let order = match order_handlers(&handlers, &builtin) {
        Ok(o) => o,
        Err(errors) => {
            for e in errors {
                diagnostics.push(Diagnostic::error(site, e));
            }
            out.extend(diagnostics.to_tokens());
            return out;
        }
    };

    let mut facts = FactStore::new();
    facts.insert(state);
    for (_, imported_id, value) in imported {
        facts.insert_boxed(imported_id, value);
    }
    let mut cx = GlobalCx::new(plenary, site, ExportableInfo::of_bundle(bundle));
    let mut produced = HandlerOutput::default();
    for reg in order {
        let inputs_ready = reg.handler.inputs().iter().all(|i| facts.has(i.id));
        if !inputs_ready {
            // A producer failed; its error was reported. Skip silently.
            continue;
        }
        run_handler(reg, &mut facts, &mut cx, &mut produced);
    }

    // One round over the emitted items: the collection site is itself a
    // source for the collections that list it (SPEC section 10).
    let round = collect_round(bundle, &produced.items, site, export);
    out.extend(round.items);
    for ob in produced.obligations {
        out.extend(ob);
    }

    // The stage carrier: exported facts plus the records of the emitted items.
    let desc = SourceDesc { export, failed: round.failed, stage: true, plugins: plugin_versions(bundle), facts: produced.exports };
    let (unconditional, conditional): (Vec<&Record>, Vec<&Record>) = round.records.iter().partition(|r| r.condition.is_none());
    let (unconditional, conditional) = if round.failed { (Vec::new(), Vec::new()) } else { (unconditional, conditional) };
    let hash = digest(&format!("{} stage {}", span_key(site), raw_input));
    out.extend(carrier_items(&hash, export, &desc, &unconditional, &conditional));

    out.extend(diagnostics.to_tokens());
    out.extend(produced.diagnostics.to_tokens());
    out.extend(round.diagnostics.to_tokens());
    out
}

/// R10: a record's plugin must have the same major version as the bundle's.
/// Returns `false` when the source's records must be dropped.
fn check_plugins(bundle: &Bundle, info: &SourceInfo, site: Span, diagnostics: &mut Diagnostics) -> bool {
    let mut keep = true;
    for pv in &info.plugins {
        let Some(plugin) = bundle.plugin_for(&pv.namespace) else { continue };
        if major_of(&pv.version) != plugin.major_version() {
            diagnostics.error(
                info.path.first().map(|p| p.span()).unwrap_or(site),
                format!(
                    "{} `{}` was collected by plugin `{}` v{}, but this bundle has `{}` v{} (major versions differ)",
                    if info.stage { "stage" } else { "source" },
                    info.display_path(),
                    pv.name,
                    pv.version,
                    plugin.name(),
                    plugin.version()
                ),
            );
            keep = false;
        }
    }
    keep
}

/// R9: records from namespaces (or kinds) this bundle doesn't know are ignored.
fn known_record(bundle: &Bundle, record: &Record) -> bool {
    match bundle.plugin_for(&record.kind.ns.to_string()) {
        Some(p) => p.kind(&record.kind.kind.to_string()).is_some(),
        None => false,
    }
}
