// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use wit_bindgen_core::wit_parser::decoding::{DecodedWasm, decode};
use wit_bindgen_core::wit_parser::{
    Function, Handle, InterfaceId, ParseError, Resolve, ResolveError, Stability, Type, TypeDefKind,
    TypeId, TypeOwner, WorldId, WorldItem,
};

use crate::{BuildError, BuildErrorKind, BuildLimits, BuildRequest, ComponentKind};

pub(super) struct RequestedWorld {
    pub resolve: Resolve,
    pub world: WorldId,
}

impl RequestedWorld {
    pub fn resolve(
        request: &BuildRequest,
        dependencies: &[String],
        limits: &BuildLimits,
    ) -> Result<Self> {
        Self::resolve_inner(request, dependencies, limits).map_err(|error| {
            BuildError::explain(&error, BuildErrorKind::InvalidWit, limits.diagnostics_bytes).into()
        })
    }

    fn resolve_inner(
        request: &BuildRequest,
        dependencies: &[String],
        limits: &BuildLimits,
    ) -> Result<Self> {
        let size = dependencies
            .iter()
            .try_fold(request.wit.len(), |size, source| {
                size.checked_add(source.len()).context("WIT size overflow")
            })?;
        ensure!(
            dependencies.len() <= 32 && size <= limits.wit_bytes,
            "combined WIT profile exceeds budget"
        );
        let mut resolve = Resolve::default();
        for (index, source) in dependencies.iter().enumerate() {
            resolve
                .push_str(format!("dependency-{index}.wit"), source)
                .map_err(|_| BuildError::new(BuildErrorKind::Unavailable))?;
        }
        let package = resolve
            .push_str("request.wit", &request.wit)
            .map_err(|error| Self::wit_error(&resolve, error, limits.diagnostics_bytes))?;
        let world = resolve
            .select_world(&[package], Some(&request.world))
            .map_err(|error| Self::wit_error(&resolve, error, limits.diagnostics_bytes))?;
        ensure!(
            resolve.types.len() <= 4096
                && resolve.interfaces.len() <= 256
                && resolve.worlds.len() <= 128,
            "WIT graph exceeds profile complexity budget"
        );
        let info = &resolve.worlds[world];
        ensure!(
            info.exports.len() <= 256 && info.imports.len() <= 256,
            "WIT world exceeds interface budget"
        );
        let mut agent = false;
        let mut client = false;
        for key in info.exports.keys() {
            let name = resolve.name_world_key(key);
            if name.starts_with("wassette:acp/") {
                ensure!(
                    request.kind == ComponentKind::AcpLayer,
                    "ACP exports require AcpLayer kind"
                );
                match name.split('@').next() {
                    Some("wassette:acp/agent") => agent = true,
                    Some("wassette:acp/client") => client = true,
                    _ => bail!("unsupported ACP export in WIT"),
                }
            }
        }
        if request.kind == ComponentKind::AcpLayer {
            ensure!(
                agent && client,
                "ACP layer WIT must export both agent and client; providers are not supported"
            );
        }
        Ok(Self { resolve, world })
    }

    fn wit_error(resolve: &Resolve, error: anyhow::Error, cap: usize) -> BuildError {
        let span = error.chain().find_map(|cause| {
            cause
                .downcast_ref::<ParseError>()
                .map(|error| error.kind().span())
                .or_else(|| {
                    cause
                        .downcast_ref::<ResolveError>()
                        .map(|error| error.kind().span())
                })
        });
        let location = span
            .filter(|span| {
                resolve
                    .source_map
                    .resolve_span(*span)
                    .is_some_and(|location| location.path == "request.wit")
            })
            .map(|span| resolve.render_location(span));
        let error = if let Some(location) = location {
            error.context(location)
        } else {
            error
        };
        BuildError::explain(&error, BuildErrorKind::InvalidWit, cap)
    }

    /// Decode actual binary types, not the guest's component-type custom section.
    /// Import implementations may be unused and elided, but every remaining
    /// import must be a correctly typed subset of the requested/runtime world.
    pub fn validate(&mut self, wasm: &[u8], limits: &BuildLimits, deadline: Instant) -> Result<()> {
        ensure!(
            wasm.len() <= limits.wasm_bytes,
            "captured component exceeds budget"
        );
        ensure!(
            Instant::now() < deadline,
            "deadline exceeded validating requested world"
        );
        wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all())
            .validate_all(wasm)
            .context("captured component is invalid WebAssembly")?;
        let DecodedWasm::Component(mut actual, actual_world) =
            decode(wasm).context("decode captured component's actual world")?
        else {
            bail!("compiler returned a WIT package, not an executable component");
        };
        for resolve in [&self.resolve, &actual] {
            ensure!(
                resolve.types.len() <= 16384
                    && resolve.interfaces.len() <= 512
                    && resolve.worlds.len() <= 128,
                "captured WIT graph exceeds validation budget"
            );
        }
        ensure!(
            actual.worlds[actual_world].imports.len() <= 320
                && actual.worlds[actual_world].exports.len() <= 256,
            "captured world exceeds import/export budget"
        );
        // An imported and exported instance of the same interface have distinct
        // nominal resources. Normalize both graphs before comparing identities.
        self.resolve.generate_nominal_type_ids(self.world);
        actual.generate_nominal_type_ids(actual_world);
        let wanted_exports = items(&self.resolve, self.world, true);
        let actual_exports = items(&actual, actual_world, true);
        ensure!(
            wanted_exports.keys().eq(actual_exports.keys()),
            "captured exports do not exactly match requested world"
        );
        let mut checker = TypeChecker::new(&self.resolve, &actual, deadline, None);
        for (name, expected) in wanted_exports {
            checker
                .item(expected, actual_exports[&name], true)
                .with_context(|| format!("captured export {name} does not match requested WIT"))?;
        }
        let wanted_imports = items(&self.resolve, self.world, false);
        let actual_imports = items(&actual, actual_world, false);
        let runtime = runtime_world()?;
        let runtime_imports = items(&runtime.resolve, runtime.world, false);
        let mut runtime_checkers = HashMap::new();
        for (name, item) in actual_imports {
            if let Some(expected) = wanted_imports.get(&name) {
                checker.item(expected, item, false).with_context(|| {
                    format!("captured import {name} does not match requested WIT")
                })?;
                continue;
            }
            let (interface, version) = name
                .rsplit_once('@')
                .context("captured import is not in the requested or runtime world")?;
            let patch = (0..=12)
                .find(|patch| version == format!("0.2.{patch}"))
                .context("captured import uses an unadmitted runtime version")?;
            let expected = runtime_imports
                .get(&format!("{interface}@0.2.12"))
                .with_context(|| format!("unadmitted captured import {name}"))?;
            let runtime_checker = runtime_checkers.entry(patch).or_insert_with(|| {
                TypeChecker::new(&runtime.resolve, &actual, deadline, Some(patch))
            });
            runtime_checker
                .item(expected, item, false)
                .with_context(|| {
                    format!("captured runtime import {name} has an unadmitted type or function")
                })?;
        }
        Ok(())
    }
}

fn items(resolve: &Resolve, world: WorldId, exports: bool) -> BTreeMap<String, &WorldItem> {
    let info = &resolve.worlds[world];
    let items = if exports {
        &info.exports
    } else {
        &info.imports
    };
    items
        .iter()
        .map(|(key, item)| (resolve.name_world_key(key), item))
        .collect()
}

fn runtime_world() -> Result<RequestedWorld> {
    let mut resolve = Resolve::default();
    for (name, source) in [
        ("io", include_str!("../../runtime/wasi-p2/io.wit")),
        ("clocks", include_str!("../../runtime/wasi-p2/clocks.wit")),
        (
            "filesystem",
            include_str!("../../runtime/wasi-p2/filesystem.wit"),
        ),
        ("sockets", include_str!("../../runtime/wasi-p2/sockets.wit")),
        ("random", include_str!("../../runtime/wasi-p2/random.wit")),
        ("cli", include_str!("../../runtime/wasi-p2/cli.wit")),
    ] {
        resolve
            .push_str(format!("runtime-{name}.wit"), source)
            .context("invalid packaged runtime WIT")?;
    }
    let package = resolve.push_str("runtime.wit", "package wassette:builder-runtime@1.0.0; world runtime { include wasi:cli/imports@0.2.12; }")?;
    let world = resolve.select_world(&[package], Some("runtime"))?;
    Ok(RequestedWorld { resolve, world })
}

struct TypeChecker<'a> {
    expected: &'a Resolve,
    actual: &'a Resolve,
    deadline: Instant,
    runtime_patch: Option<u64>,
    remaining: usize,
    seen: HashSet<(TypeId, TypeId)>,
    resources: HashMap<TypeId, TypeId>,
    reverse_resources: HashMap<TypeId, TypeId>,
}

impl<'a> TypeChecker<'a> {
    fn new(
        expected: &'a Resolve,
        actual: &'a Resolve,
        deadline: Instant,
        runtime_patch: Option<u64>,
    ) -> Self {
        Self {
            expected,
            actual,
            deadline,
            runtime_patch,
            remaining: 100_000,
            seen: HashSet::new(),
            resources: HashMap::new(),
            reverse_resources: HashMap::new(),
        }
    }

    fn step(&mut self, depth: usize) -> Result<()> {
        ensure!(
            Instant::now() < self.deadline,
            "deadline exceeded comparing WIT types"
        );
        ensure!(
            depth <= 128 && self.remaining > 0,
            "WIT type comparison exceeds complexity budget"
        );
        self.remaining -= 1;
        Ok(())
    }

    fn admitted_stability(&self, stability: &Stability) -> Result<()> {
        if let Some(patch) = self.runtime_patch {
            match stability {
                Stability::Stable { since, .. } => ensure!(
                    (since.major, since.minor, since.patch) <= (0, 2, patch),
                    "runtime member is newer than the admitted import version"
                ),
                Stability::Unstable { .. } => bail!("unstable runtime member is not admitted"),
                Stability::Unknown => {}
            }
        }
        Ok(())
    }

    fn item(&mut self, expected: &WorldItem, actual: &WorldItem, exact: bool) -> Result<()> {
        self.step(0)?;
        match (expected, actual) {
            (WorldItem::Interface { id: e, .. }, WorldItem::Interface { id: a, .. }) => {
                self.interface(*e, *a, exact)
            }
            (WorldItem::Function(e), WorldItem::Function(a)) => self.function(e, a),
            (WorldItem::Type { id: e, .. }, WorldItem::Type { id: a, .. }) => {
                self.ty(Type::Id(*e), Type::Id(*a), 0)
            }
            _ => bail!("different world item kinds"),
        }
    }

    fn interface(&mut self, expected: InterfaceId, actual: InterfaceId, exact: bool) -> Result<()> {
        let e = &self.expected.interfaces[expected];
        let a = &self.actual.interfaces[actual];
        self.admitted_stability(&e.stability)?;
        if exact {
            ensure!(
                e.types.len() == a.types.len() && e.functions.len() == a.functions.len(),
                "exported interface has missing or extra functions/types"
            );
        }
        for (name, ty) in &a.types {
            let expected = e
                .types
                .get(name)
                .with_context(|| format!("unadmitted type {name}"))?;
            self.ty(Type::Id(*expected), Type::Id(*ty), 0)
                .with_context(|| format!("type {name}"))?;
        }
        for (name, function) in &a.functions {
            let expected = e
                .functions
                .get(name)
                .with_context(|| format!("unadmitted function {name}"))?;
            self.function(expected, function)
                .with_context(|| format!("function {name}"))?;
        }
        Ok(())
    }

    fn function(&mut self, expected: &Function, actual: &Function) -> Result<()> {
        self.step(0)?;
        self.admitted_stability(&expected.stability)?;
        ensure!(
            std::mem::discriminant(&expected.kind) == std::mem::discriminant(&actual.kind),
            "function async/resource kind differs"
        );
        ensure!(
            expected.params.len() == actual.params.len(),
            "function parameter count differs"
        );
        if let (Some(e), Some(a)) = (expected.kind.resource(), actual.kind.resource()) {
            self.ty(Type::Id(e), Type::Id(a), 0)?;
        }
        for (e, a) in expected.params.iter().zip(&actual.params) {
            ensure!(e.name == a.name, "function parameter name differs");
            self.ty(e.ty, a.ty, 0)?;
        }
        self.optional(expected.result, actual.result, 0)
    }

    fn optional(
        &mut self,
        expected: Option<Type>,
        actual: Option<Type>,
        depth: usize,
    ) -> Result<()> {
        match (expected, actual) {
            (Some(e), Some(a)) => self.ty(e, a, depth + 1),
            (None, None) => Ok(()),
            _ => bail!("optional type/result presence differs"),
        }
    }

    fn ty(&mut self, expected: Type, actual: Type, depth: usize) -> Result<()> {
        self.step(depth)?;
        if let Type::Id(e) = expected {
            self.admitted_stability(&self.expected.types[e].stability)?;
            if let TypeDefKind::Type(inner) = self.expected.types[e].kind {
                return self.ty(inner, actual, depth + 1);
            }
        }
        if let Type::Id(a) = actual
            && let TypeDefKind::Type(inner) = self.actual.types[a].kind
        {
            return self.ty(expected, inner, depth + 1);
        }
        let (Type::Id(e), Type::Id(a)) = (expected, actual) else {
            ensure!(expected == actual, "primitive type differs");
            return Ok(());
        };
        if !self.seen.insert((e, a)) {
            return Ok(());
        }
        use TypeDefKind as K;
        match (&self.expected.types[e].kind, &self.actual.types[a].kind) {
            (K::Resource, K::Resource) => {
                let old = self.resources.insert(e, a);
                let reverse = self.reverse_resources.insert(a, e);
                ensure!(
                    old.is_none_or(|id| id == a) && reverse.is_none_or(|id| id == e),
                    "nominal resource identity differs"
                );
                ensure!(
                    self.expected.types[e].name == self.actual.types[a].name,
                    "resource name differs"
                );
                match (self.expected.types[e].owner, self.actual.types[a].owner) {
                    (TypeOwner::Interface(e), TypeOwner::Interface(a)) => {
                        let e = self.expected.id_of(e);
                        let a = self.actual.id_of(a);
                        if self.runtime_patch.is_some() {
                            ensure!(
                                e.as_deref().map(|s| s.split('@').next())
                                    == a.as_deref().map(|s| s.split('@').next()),
                                "runtime resource owner differs"
                            );
                        } else {
                            ensure!(e == a, "resource interface owner differs");
                        }
                    }
                    (TypeOwner::World(_), TypeOwner::World(_)) => {}
                    (TypeOwner::None, TypeOwner::None) => {}
                    _ => bail!("resource owner kind differs"),
                }
            }
            (K::Handle(Handle::Own(e)), K::Handle(Handle::Own(a)))
            | (K::Handle(Handle::Borrow(e)), K::Handle(Handle::Borrow(a))) => {
                self.ty(Type::Id(*e), Type::Id(*a), depth + 1)?
            }
            (K::Record(e), K::Record(a)) => {
                ensure!(
                    e.fields.len() == a.fields.len(),
                    "record field count differs"
                );
                for (e, a) in e.fields.iter().zip(&a.fields) {
                    ensure!(e.name == a.name, "record field name/order differs");
                    self.ty(e.ty, a.ty, depth + 1)?;
                }
            }
            (K::Flags(e), K::Flags(a)) => ensure!(
                e.flags
                    .iter()
                    .map(|f| &f.name)
                    .eq(a.flags.iter().map(|f| &f.name)),
                "flag names/order differ"
            ),
            (K::Enum(e), K::Enum(a)) => ensure!(
                e.cases
                    .iter()
                    .map(|c| &c.name)
                    .eq(a.cases.iter().map(|c| &c.name)),
                "enum cases/order differ"
            ),
            (K::Variant(e), K::Variant(a)) => {
                ensure!(e.cases.len() == a.cases.len(), "variant case count differs");
                for (e, a) in e.cases.iter().zip(&a.cases) {
                    ensure!(e.name == a.name, "variant case name/order differs");
                    self.optional(e.ty, a.ty, depth + 1)?;
                }
            }
            (K::Tuple(e), K::Tuple(a)) => {
                ensure!(e.types.len() == a.types.len(), "tuple length differs");
                for (e, a) in e.types.iter().zip(&a.types) {
                    self.ty(*e, *a, depth + 1)?;
                }
            }
            (K::Option(e), K::Option(a)) | (K::List(e), K::List(a)) => {
                self.ty(*e, *a, depth + 1)?
            }
            (K::FixedLengthList(e, el), K::FixedLengthList(a, al)) => {
                ensure!(el == al, "fixed-length list size differs");
                self.ty(*e, *a, depth + 1)?;
            }
            (K::Map(ek, ev), K::Map(ak, av)) => {
                self.ty(*ek, *ak, depth + 1)?;
                self.ty(*ev, *av, depth + 1)?;
            }
            (K::Result(e), K::Result(a)) => {
                self.optional(e.ok, a.ok, depth + 1)?;
                self.optional(e.err, a.err, depth + 1)?;
            }
            (K::Future(e), K::Future(a)) | (K::Stream(e), K::Stream(a)) => {
                self.optional(*e, *a, depth + 1)?
            }
            _ => bail!("type definition kind differs"),
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "world_tests.rs"]
mod tests;
