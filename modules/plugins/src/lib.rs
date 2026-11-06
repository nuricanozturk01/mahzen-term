//! WebAssembly plugins, only the ones the user names.
//!
//! A plugin is a folder `<config>/plugins/<name>/` with `plugin.toml` and a `.wasm` module. Nothing runs unless the
//! name is listed in `[modules.plugins] allow = [...]`: discovery alone never executes code. Plugins are sandboxed
//! by the WebAssembly runtime (`wasmi`, a pure interpreter): they get no files, network, processes or clock beyond
//! the two host functions below, and every call is bounded in CPU (fuel), memory and output size.
//!
//! **ABI v1** (text based, so any language that targets wasm32 can write a plugin):
//! - exports `memory`, `mz_alloc(len: i32) -> i32` and `mz_render(ptr: i32, len: i32) -> i64`;
//! - the host writes `width=<n>` at the pointer it got from `mz_alloc` and calls `mz_render`;
//! - the result packs a UTF-8 text as `(ptr << 32) | len` made of lines: `title=`, `badge=`, `empty=` and
//!   `row=<tone>|<label>|<value>` with tone one of `normal dim good warn bad accent`;
//! - imports (module `mz`): `log(ptr: i32, len: i32)` and `now_ms() -> i64`.
//!
//! The text becomes an info card, so a plugin panel can be a card, a tab or a window like any built-in widget.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use mahzenterm_core::command::CommandSpec;
use mahzenterm_core::module::{Capability, Module, ModuleManifest};
use mahzenterm_ui_kit::infocard::MAX_ROWS;
use mahzenterm_ui_kit::{InfoCard, InfoRow, InfoTone};
use serde::Deserialize;
use wasmi::{
    Engine, Instance, Linker, Memory, Module as WasmModule, Store, StoreLimits, StoreLimitsBuilder,
    TypedFunc,
};

/// Most plugin panels (each gets a card slot).
pub const MAX_PLUGINS: usize = 4;
const MAX_WASM_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MEMORY_BYTES: usize = 16 * 1024 * 1024;
/// CPU budget of one call (wasmi fuel units, roughly instructions).
const FUEL_PER_CALL: u64 = 20_000_000;
const MAX_OUTPUT: usize = 16 * 1024;
const MAX_LINE: usize = 200;

/// Command ids of this module.
pub mod id {
    pub const RESCAN: &str = "plugins.rescan";
}

/// `[modules.plugins]` in the config.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Plugin folder names that may run. Everything else is ignored.
    pub allow: Vec<String>,
}

/// `plugin.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// The module file inside the plugin folder.
    #[serde(default = "default_wasm")]
    pub wasm: String,
    /// Seconds between renders while the panel is shown (at least 1).
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    /// Reserved: no capability beyond drawing a panel is available yet, so a plugin asking for any is refused.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

fn default_wasm() -> String {
    "plugin.wasm".into()
}

fn default_interval() -> u64 {
    5
}

/// Why a plugin could not be loaded or run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginError(pub String);

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(msg: impl Into<String>) -> Result<T, PluginError> {
    Err(PluginError(msg.into()))
}

struct HostState {
    limits: StoreLimits,
    name: String,
}

struct Runtime {
    store: Store<HostState>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    render: TypedFunc<(i32, i32), i64>,
}

/// A loaded, allowed plugin.
pub struct Plugin {
    pub manifest: Manifest,
    engine: Engine,
    module: WasmModule,
    runtime: Mutex<Option<Runtime>>,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugin")
            .field("name", &self.manifest.name)
            .finish_non_exhaustive()
    }
}

fn safe_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn safe_file(n: &str) -> bool {
    n.ends_with(".wasm") && !n.contains(['/', '\\']) && !n.starts_with('.') && n.len() <= 100
}

impl Plugin {
    /// Load the plugin in `dir` (its manifest and module are validated; no code runs yet).
    pub fn load(dir: &Path) -> Result<Self, PluginError> {
        let text = std::fs::read_to_string(dir.join("plugin.toml"))
            .map_err(|e| PluginError(format!("plugin.toml: {e}")))?;
        let manifest: Manifest = toml::from_str(&text)
            .map_err(|e| PluginError(format!("plugin.toml: {}", e.message())))?;
        if !safe_name(&manifest.name) {
            return err("plugin name may only use letters, digits, - and _");
        }
        if dir.file_name().and_then(|n| n.to_str()) != Some(manifest.name.as_str()) {
            return err("the folder name must equal the plugin name");
        }
        if !safe_file(&manifest.wasm) {
            return err("the wasm file must be a plain .wasm file name inside the plugin folder");
        }
        if !manifest.capabilities.is_empty() {
            return err(format!(
                "capabilities are not supported yet (asked for: {})",
                manifest.capabilities.join(", ")
            ));
        }
        let path = dir.join(&manifest.wasm);
        let len = std::fs::metadata(&path)
            .map_err(|e| PluginError(format!("{}: {e}", manifest.wasm)))?
            .len();
        if len > MAX_WASM_BYTES {
            return err("the wasm file is larger than 4 MiB");
        }
        let bytes = std::fs::read(&path).map_err(|e| PluginError(e.to_string()))?;
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = WasmModule::new(&engine, &bytes[..])
            .map_err(|e| PluginError(format!("not a valid wasm module: {e}")))?;
        Ok(Self {
            manifest,
            engine,
            module,
            runtime: Mutex::new(None),
        })
    }

    pub fn title(&self) -> &str {
        if self.manifest.title.is_empty() {
            &self.manifest.name
        } else {
            &self.manifest.title
        }
    }

    pub fn interval_secs(&self) -> u64 {
        self.manifest.interval_secs.max(1)
    }

    fn start(&self) -> Result<Runtime, PluginError> {
        let mut store = Store::new(
            &self.engine,
            HostState {
                limits: StoreLimitsBuilder::new()
                    .memory_size(MAX_MEMORY_BYTES)
                    .instances(1)
                    .memories(1)
                    .tables(2)
                    .build(),
                name: self.manifest.name.clone(),
            },
        );
        store.limiter(|s| &mut s.limits);
        store
            .set_fuel(FUEL_PER_CALL)
            .map_err(|e| PluginError(e.to_string()))?;
        let mut linker: Linker<HostState> = Linker::new(&self.engine);
        linker
            .func_wrap(
                "mz",
                "log",
                |caller: wasmi::Caller<'_, HostState>, ptr: i32, len: i32| {
                    if let Some(mem) = caller
                        .get_export("memory")
                        .and_then(wasmi::Extern::into_memory)
                    {
                        let n = usize::try_from(len).unwrap_or(0).min(500);
                        let mut buf = vec![0u8; n];
                        if mem
                            .read(
                                &caller,
                                usize::try_from(ptr).unwrap_or(usize::MAX),
                                &mut buf,
                            )
                            .is_ok()
                        {
                            tracing::info!(
                                plugin = caller.data().name,
                                "{}",
                                String::from_utf8_lossy(&buf)
                            );
                        }
                    }
                },
            )
            .map_err(|e| PluginError(e.to_string()))?;
        linker
            .func_wrap("mz", "now_ms", |_: wasmi::Caller<'_, HostState>| -> i64 {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
            })
            .map_err(|e| PluginError(e.to_string()))?;
        let instance: Instance = linker
            .instantiate_and_start(&mut store, &self.module)
            .map_err(|e| PluginError(format!("cannot start the plugin: {e}")))?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| PluginError("the plugin exports no `memory`".into()))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&store, "mz_alloc")
            .map_err(|e| PluginError(format!("missing export mz_alloc(i32) -> i32: {e}")))?;
        let render = instance
            .get_typed_func::<(i32, i32), i64>(&store, "mz_render")
            .map_err(|e| PluginError(format!("missing export mz_render(i32, i32) -> i64: {e}")))?;
        Ok(Runtime {
            store,
            memory,
            alloc,
            render,
        })
    }

    /// Ask the plugin for its panel (blocking; call from a background task). A plugin that traps or breaks a limit
    /// is restarted on the next call and the error is returned.
    pub fn render(&self, width: u32) -> Result<InfoCard, PluginError> {
        let mut guard = self
            .runtime
            .lock()
            .map_err(|_| PluginError("plugin state is poisoned".into()))?;
        if guard.is_none() {
            *guard = Some(self.start()?);
        }
        let rt = guard
            .as_mut()
            .ok_or_else(|| PluginError("plugin did not start".into()))?;
        let result = Self::call(rt, width);
        if result.is_err() {
            *guard = None; // a broken instance is thrown away
        }
        let text = result?;
        Ok(parse_card(&text, self.title()))
    }

    fn call(rt: &mut Runtime, width: u32) -> Result<String, PluginError> {
        let trap = |what: &str, e: wasmi::Error| PluginError(format!("{what}: {e}"));
        rt.store
            .set_fuel(FUEL_PER_CALL)
            .map_err(|e| PluginError(e.to_string()))?;
        let input = format!("width={width}");
        let len = i32::try_from(input.len()).map_err(|_| PluginError("input too long".into()))?;
        let ptr = rt
            .alloc
            .call(&mut rt.store, len)
            .map_err(|e| trap("mz_alloc", e))?;
        rt.memory
            .write(
                &mut rt.store,
                usize::try_from(ptr).map_err(|_| PluginError("bad pointer".into()))?,
                input.as_bytes(),
            )
            .map_err(|e| PluginError(format!("cannot write input: {e}")))?;
        let packed = rt
            .render
            .call(&mut rt.store, (ptr, len))
            .map_err(|e| trap("mz_render", e))?;
        let (out_ptr, out_len) = (
            (packed >> 32) as u32 as usize,
            (packed & 0xffff_ffff) as usize,
        );
        if out_len > MAX_OUTPUT {
            return err("the plugin answered with more than 16 KiB");
        }
        let mut buf = vec![0u8; out_len];
        rt.memory
            .read(&rt.store, out_ptr, &mut buf)
            .map_err(|_| PluginError("the plugin returned a bad pointer".into()))?;
        String::from_utf8(buf)
            .map_err(|_| PluginError("the plugin answered with invalid UTF-8".into()))
    }
}

fn tone(s: &str) -> InfoTone {
    match s.trim() {
        "dim" => InfoTone::Dim,
        "good" => InfoTone::Good,
        "warn" => InfoTone::Warn,
        "bad" => InfoTone::Bad,
        "accent" => InfoTone::Accent,
        _ => InfoTone::Normal,
    }
}

fn clip(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(MAX_LINE)
        .collect()
}

/// Turn a plugin's answer into a card. Unknown lines are ignored; limits apply to rows and line length.
pub fn parse_card(text: &str, default_title: &str) -> InfoCard {
    let mut card = InfoCard {
        title: default_title.to_owned(),
        ..InfoCard::default()
    };
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("title=") {
            card.title = clip(v);
        } else if let Some(v) = line.strip_prefix("badge=") {
            card.badge = clip(v);
        } else if let Some(v) = line.strip_prefix("empty=") {
            card.empty = clip(v);
        } else if let Some(v) = line.strip_prefix("row=") {
            if card.rows.len() < MAX_ROWS {
                let mut parts = v.splitn(3, '|');
                if let (Some(t), Some(l), Some(val)) = (parts.next(), parts.next(), parts.next()) {
                    card.rows.push(InfoRow::new(clip(l), clip(val), tone(t)));
                }
            }
        }
    }
    if card.rows.is_empty() && card.empty.is_empty() {
        card.empty = "nothing to show".into();
    }
    card
}

/// What discovery found.
#[derive(Debug, Default)]
pub struct Scan {
    /// Allowed plugins that loaded, in the order of the allow-list (at most [`MAX_PLUGINS`]).
    pub loaded: Vec<Plugin>,
    /// Problems with allowed plugins (shown to the user).
    pub problems: Vec<String>,
    /// Plugin folders present but not allowed (they never run).
    pub not_allowed: Vec<String>,
}

/// Look in `dir` and load exactly the plugins named in `settings.allow`.
pub fn scan(dir: &Path, settings: &Settings) -> Scan {
    let mut out = Scan::default();
    let present: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    for name in settings.allow.iter().take(MAX_PLUGINS) {
        if !safe_name(name) {
            out.problems
                .push(format!("plugin \"{name}\": the name is not valid"));
            continue;
        }
        if !present.contains(name) {
            out.problems.push(format!(
                "plugin \"{name}\" is allowed but not found in {}",
                dir.display()
            ));
            continue;
        }
        match Plugin::load(&dir.join(name)) {
            Ok(p) => out.loaded.push(p),
            Err(e) => out.problems.push(format!("plugin \"{name}\": {e}")),
        }
    }
    if settings.allow.len() > MAX_PLUGINS {
        out.problems.push(format!(
            "only the first {MAX_PLUGINS} allowed plugins are loaded"
        ));
    }
    out.not_allowed = present
        .into_iter()
        .filter(|n| !settings.allow.contains(n))
        .collect();
    out.not_allowed.sort();
    out
}

/// The default plugin folder under the config directory.
pub fn plugins_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("plugins")
}

/// The module registration.
pub struct PluginsModule;

impl Module for PluginsModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "plugins",
            name: "Plugins",
            version: env!("CARGO_PKG_VERSION"),
            capabilities: vec![Capability::FsRead],
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![CommandSpec::new(
            id::RESCAN,
            "Plugins: Reload Allowed Plugins",
            "Plugins",
        )]
    }
}

#[cfg(test)]
mod tests;
