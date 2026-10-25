//! The plugin API: namespaces, kinds, local and global handlers, facts.

use crate::facts::ExportFact;
use crate::global::GlobalCx;
use crate::local::LocalCx;
use crate::record::Record;
use crate::Result;
use proc_macro2::TokenStream;
use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fmt;

/// A local handler: runs inside `collect`, once per record of its kind, and
/// sees only that record (R2).
pub type LocalHandler = fn(&Record, &mut LocalCx) -> Result<()>;

/// The default local handler: keeps the item as written (with plugin
/// attributes stripped) and attaches nothing.
pub fn keep(_record: &Record, _cx: &mut LocalCx) -> Result<()> {
    Ok(())
}

/// A plain Rust value that one global handler produces and others read during
/// one collection run. Facts are looked up by type.
///
/// Implement it for your fact types: `impl plenary_plugin::Fact for Resolved {}`.
/// [`State`](crate::State) is a built-in fact.
pub trait Fact: Any + 'static {}

/// What a global handler may return inside `Result`: a [`Fact`], or `()` for
/// no fact.
pub trait FactOutput: 'static {
    #[doc(hidden)]
    fn fact_id() -> Option<FactId>;
    #[doc(hidden)]
    fn into_fact(self) -> Option<Box<dyn Any>>;
}

impl FactOutput for () {
    fn fact_id() -> Option<FactId> {
        None
    }
    fn into_fact(self) -> Option<Box<dyn Any>> {
        None
    }
}

impl<T: Fact> FactOutput for T {
    fn fact_id() -> Option<FactId> {
        Some(FactId::of::<T>())
    }
    fn into_fact(self) -> Option<Box<dyn Any>> {
        Some(Box::new(self))
    }
}

/// Identifies a fact type: its `TypeId` plus a readable name for diagnostics.
#[derive(Clone, Copy, Debug)]
pub struct FactId {
    pub(crate) id: TypeId,
    pub(crate) name: &'static str,
}

impl FactId {
    pub fn of<T: 'static>() -> Self {
        FactId { id: TypeId::of::<T>(), name: std::any::type_name::<T>() }
    }

    /// `std::any::type_name` of the fact type.
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl PartialEq for FactId {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for FactId {}

/// The facts produced so far in one collection run.
#[derive(Default)]
pub struct FactStore {
    facts: std::collections::HashMap<TypeId, Box<dyn Any>>,
}

impl FactStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a fact. Returns `false` if one of that type already exists.
    pub fn insert<T: Fact>(&mut self, fact: T) -> bool {
        self.insert_boxed(TypeId::of::<T>(), Box::new(fact))
    }

    pub(crate) fn insert_boxed(&mut self, id: TypeId, fact: Box<dyn Any>) -> bool {
        if self.facts.contains_key(&id) {
            return false;
        }
        self.facts.insert(id, fact);
        true
    }

    /// Look a fact up by type.
    pub fn get<T: Fact>(&self) -> Option<&T> {
        self.facts.get(&TypeId::of::<T>()).and_then(|b| b.downcast_ref::<T>())
    }

    pub(crate) fn has(&self, id: TypeId) -> bool {
        self.facts.contains_key(&id)
    }
}

/// Type-erased global handler. Produced by [`GlobalHandler::erase`].
pub trait ErasedGlobalHandler {
    /// `std::any::type_name` of the function.
    fn name(&self) -> &'static str;
    /// The facts the handler reads, in parameter order.
    fn inputs(&self) -> Vec<FactId>;
    /// The fact the handler produces, if any.
    fn output(&self) -> Option<FactId>;
    /// Run the handler. The store must hold every input.
    fn run(&self, facts: &FactStore, cx: &mut GlobalCx) -> Result<Option<Box<dyn Any>>>;
}

/// Implemented for functions of the shape
/// `fn(&A, &B, ..., &mut GlobalCx) -> Result<T>` with 0 to 6 fact inputs,
/// where every input is a [`Fact`] and `T` is a [`Fact`] or `()`.
///
/// The `Marker` parameter only exists so that one function type can't match
/// two impls; it is inferred.
pub trait GlobalHandler<Marker> {
    /// Erase the function's type so the engine can store it.
    fn erase(self) -> Box<dyn ErasedGlobalHandler>;
}

macro_rules! impl_global_handler {
    ($($A:ident),*) => {
        impl<F, R, $($A,)*> GlobalHandler<fn($($A,)*) -> R> for F
        where
            F: Fn($(&$A,)* &mut GlobalCx) -> Result<R> + 'static,
            R: FactOutput,
            $($A: Fact,)*
        {
            fn erase(self) -> Box<dyn ErasedGlobalHandler> {
                struct Erased<F, R, $($A,)*>(F, std::marker::PhantomData<fn() -> (R, $($A,)*)>);
                impl<F, R, $($A,)*> ErasedGlobalHandler for Erased<F, R, $($A,)*>
                where
                    F: Fn($(&$A,)* &mut GlobalCx) -> Result<R> + 'static,
                    R: FactOutput,
                    $($A: Fact,)*
                {
                    fn name(&self) -> &'static str {
                        std::any::type_name::<F>()
                    }
                    fn inputs(&self) -> Vec<FactId> {
                        vec![$(FactId::of::<$A>()),*]
                    }
                    fn output(&self) -> Option<FactId> {
                        R::fact_id()
                    }
                    #[allow(non_snake_case, unused_variables)]
                    fn run(&self, facts: &FactStore, cx: &mut GlobalCx) -> Result<Option<Box<dyn Any>>> {
                        $(
                            let $A = facts.get::<$A>().expect("handler ordering guarantees inputs exist");
                        )*
                        let out = (self.0)($($A,)* cx)?;
                        Ok(out.into_fact())
                    }
                }
                Box::new(Erased(self, std::marker::PhantomData))
            }
        }
    };
}

impl_global_handler!();
impl_global_handler!(A);
impl_global_handler!(A, B);
impl_global_handler!(A, B, C);
impl_global_handler!(A, B, C, D);
impl_global_handler!(A, B, C, D, E);
impl_global_handler!(A, B, C, D, E, G);

/// Facts imported from the stages a collection lists (SPEC section 10).
///
/// For every type a plugin registered with [`Plugin::exportable`], the engine
/// puts an `Imported<F>` in the fact store before any handler runs, holding
/// one `(SourceInfo, F)` per exported value found in the listed stages, in
/// canonical order (stage position in the `expand!` list, then export
/// order inside the stage). It may be empty. A handler reads it by taking
/// `&Imported<F>`; a bare `&F` still means "produced by a handler in this
/// bundle" (R6).
pub struct Imported<F> {
    items: Vec<(crate::SourceInfo, F)>,
}

impl<F> Imported<F> {
    pub(crate) fn empty() -> Self {
        Imported { items: Vec::new() }
    }

    pub(crate) fn push(&mut self, source: crate::SourceInfo, fact: F) {
        self.items.push((source, fact));
    }

    /// `(stage, fact)` pairs in canonical order.
    pub fn iter(&self) -> impl Iterator<Item = (&crate::SourceInfo, &F)> + '_ {
        self.items.iter().map(|(s, f)| (s, f))
    }

    /// The facts alone, in canonical order.
    pub fn facts(&self) -> impl Iterator<Item = &F> + '_ {
        self.items.iter().map(|(_, f)| f)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl<F: ExportFact> Fact for Imported<F> {}

impl<F: fmt::Debug> fmt::Debug for Imported<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.items.iter().map(|(s, v)| (s.display_path(), v))).finish()
    }
}

/// A registered exportable fact type (type-erased).
pub struct ExportableDef {
    pub(crate) id: TypeId,
    pub(crate) imported_id: TypeId,
    pub(crate) name: &'static str,
    pub(crate) type_name: &'static str,
    decode: fn(TokenStream) -> Result<Box<dyn Any>>,
    new_imported: fn() -> Box<dyn Any>,
    push_imported: fn(&mut dyn Any, crate::SourceInfo, Box<dyn Any>),
}

impl ExportableDef {
    fn of<F: ExportFact>() -> Self {
        fn decode<F: ExportFact>(tokens: TokenStream) -> Result<Box<dyn Any>> {
            Ok(Box::new(F::from_tokens(tokens)?))
        }
        fn new_imported<F: ExportFact>() -> Box<dyn Any> {
            Box::new(Imported::<F>::empty())
        }
        fn push_imported<F: ExportFact>(imported: &mut dyn Any, source: crate::SourceInfo, fact: Box<dyn Any>) {
            let imported = imported.downcast_mut::<Imported<F>>().expect("Imported<F>");
            let fact = *fact.downcast::<F>().expect("F");
            imported.push(source, fact);
        }
        ExportableDef {
            id: TypeId::of::<F>(),
            imported_id: TypeId::of::<Imported<F>>(),
            name: F::NAME,
            type_name: std::any::type_name::<F>(),
            decode: decode::<F>,
            new_imported: new_imported::<F>,
            push_imported: push_imported::<F>,
        }
    }

    /// The name in carriers (`ExportFact::NAME`).
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// `std::any::type_name` of the fact type.
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    pub(crate) fn decode(&self, tokens: TokenStream) -> Result<Box<dyn Any>> {
        (self.decode)(tokens)
    }

    pub(crate) fn new_imported(&self) -> Box<dyn Any> {
        (self.new_imported)()
    }

    pub(crate) fn push_imported(&self, imported: &mut dyn Any, source: crate::SourceInfo, fact: Box<dyn Any>) {
        (self.push_imported)(imported, source, fact)
    }
}

/// One record kind a plugin owns.
pub struct KindDef {
    pub(crate) name: &'static str,
    pub(crate) form: crate::Form,
    pub(crate) local: LocalHandler,
}

/// A plugin: a namespace, its record kinds with their local handlers, and its
/// global handlers.
///
/// ```ignore
/// pub fn plugin() -> plenary_plugin::Plugin {
///     plenary_plugin::Plugin::new("stq", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///         .attribute_kind("table", local::table)
///         .attribute_kind("query", local::query)
///         .call_kind("sql", local::sql)
///         .global(global::link)
///         .global(global::render)
/// }
/// ```
pub struct Plugin {
    pub(crate) namespace: &'static str,
    pub(crate) name: &'static str,
    pub(crate) version: &'static str,
    pub(crate) kinds: Vec<KindDef>,
    pub(crate) globals: Vec<Box<dyn ErasedGlobalHandler>>,
    pub(crate) exportables: Vec<ExportableDef>,
}

impl Plugin {
    /// A plugin owning `namespace`. `name` and `version` identify the plugin
    /// crate in diagnostics and in compatibility checks (R10); pass
    /// `env!("CARGO_PKG_NAME")` and `env!("CARGO_PKG_VERSION")`.
    pub fn new(namespace: &'static str, name: &'static str, version: &'static str) -> Self {
        Plugin { namespace, name, version, kinds: Vec::new(), globals: Vec::new(), exportables: Vec::new() }
    }

    /// Register a fact type that stages may export and collections import
    /// (SPEC section 10). Register a type in the plugin that defines it: the
    /// carrier records this plugin's namespace and version next to the fact,
    /// and a collection matches facts by namespace and `ExportFact::NAME`.
    /// Global handlers export values with `GlobalCx::export` and read
    /// imported ones through `&Imported<F>`.
    pub fn exportable<F: ExportFact>(mut self) -> Self {
        self.exportables.push(ExportableDef::of::<F>());
        self
    }

    /// Register an attribute-form kind: `#[ns::kind(args)] item`.
    pub fn attribute_kind(mut self, kind: &'static str, local: LocalHandler) -> Self {
        self.kinds.push(KindDef { name: kind, form: crate::Form::Attribute, local });
        self
    }

    /// Register a call-form kind: `ns::kind!(args)`. Its local handler should
    /// set a replacement; with none, the call expands to nothing, which is a
    /// syntax error in expression position.
    pub fn call_kind(mut self, kind: &'static str, local: LocalHandler) -> Self {
        self.kinds.push(KindDef { name: kind, form: crate::Form::Call, local });
        self
    }

    /// Register a global handler. Handlers are ordered by the facts they read
    /// and produce (R6); registration order breaks ties.
    pub fn global<M>(mut self, handler: impl GlobalHandler<M>) -> Self {
        self.globals.push(handler.erase());
        self
    }

    pub fn namespace(&self) -> &'static str {
        self.namespace
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn version(&self) -> &'static str {
        self.version
    }

    /// The major component of the version (R10).
    pub fn major_version(&self) -> &str {
        major_of(self.version)
    }

    pub(crate) fn kind(&self, name: &str) -> Option<&KindDef> {
        self.kinds.iter().find(|k| k.name == name)
    }

    pub(crate) fn kind_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.kinds.iter().map(|k| k.name)
    }

    /// The fact types this plugin registered as exportable.
    pub fn exportables(&self) -> &[ExportableDef] {
        &self.exportables
    }
}

impl fmt::Debug for Plugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plugin")
            .field("namespace", &self.namespace)
            .field("name", &self.name)
            .field("version", &self.version)
            .field("kinds", &self.kinds.iter().map(|k| k.name).collect::<Vec<_>>())
            .field("globals", &self.globals.iter().map(|g| g.name()).collect::<Vec<_>>())
            .field("exportables", &self.exportables.iter().map(|e| e.name).collect::<Vec<_>>())
            .finish()
    }
}

pub(crate) fn major_of(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

/// What `bundle!` hands the engine: the bundle crate's name and its plugins.
pub struct BundleSpec {
    /// `env!("CARGO_CRATE_NAME")` of the bundle crate.
    pub crate_name: &'static str,
    /// The plugin constructors, in list order.
    pub plugins: &'static [fn() -> Plugin],
}

/// A loaded bundle: the plugins, with namespaces indexed.
pub struct Bundle {
    pub(crate) crate_name: String,
    pub(crate) plugins: Vec<Plugin>,
    pub(crate) namespaces: BTreeMap<String, usize>,
}

impl Bundle {
    /// Instantiate the plugins. Fails if two plugins claim one namespace.
    pub fn new(crate_name: &str, plugins: Vec<Plugin>) -> std::result::Result<Self, String> {
        let mut namespaces = BTreeMap::new();
        for (i, p) in plugins.iter().enumerate() {
            if let Some(prev) = namespaces.insert(p.namespace.to_string(), i) {
                return Err(format!(
                    "plugins `{}` and `{}` both register the namespace `{}`",
                    plugins[prev].name, p.name, p.namespace
                ));
            }
        }
        Ok(Bundle { crate_name: crate_name.to_string(), plugins, namespaces })
    }

    pub fn from_spec(spec: &BundleSpec) -> std::result::Result<Self, String> {
        Self::new(spec.crate_name, spec.plugins.iter().map(|f| f()).collect())
    }

    pub fn plugins(&self) -> &[Plugin] {
        &self.plugins
    }

    /// The plugin owning `namespace`, if the bundle knows it.
    pub fn plugin_for(&self, namespace: &str) -> Option<&Plugin> {
        self.namespaces.get(namespace).map(|&i| &self.plugins[i])
    }

    pub fn knows_namespace(&self, namespace: &str) -> bool {
        self.namespaces.contains_key(namespace)
    }

    pub fn crate_name(&self) -> &str {
        &self.crate_name
    }

    /// Every registered exportable fact type with the plugin that registered
    /// it, in bundle order.
    pub fn exportables(&self) -> impl Iterator<Item = (&Plugin, &ExportableDef)> + '_ {
        self.plugins.iter().flat_map(|p| p.exportables.iter().map(move |e| (p, e)))
    }

    /// The exportable registered under `ns`/`name`, if any.
    pub(crate) fn exportable_named(&self, ns: &str, name: &str) -> Option<(&Plugin, &ExportableDef)> {
        let plugin = self.plugin_for(ns)?;
        plugin.exportables.iter().find(|e| e.name == name).map(|e| (plugin, e))
    }

}
