//! The one place the CLI turns a single-file input into frontend compiler
//! inputs.
//!
//! `check`, `fix`, `build`, `run` and `inspect` all compile the same thing: one
//! source file, the language's standard package interfaces, and whatever
//! `--interface` files the caller named. They used to assemble that separately,
//! and they drifted: `check` attached the standard package interfaces while
//! `build`/`run`/`inspect` attached none, so `channel-pipeline.rss` checked
//! clean and then failed to build with sixteen diagnostics. Every command now
//! reads its input through [`SourceInput`], so the assembly cannot diverge
//! again without changing this file.

use std::collections::BTreeSet;
use std::path::Path;

use rsscript_diagnostics::Diagnostic;
#[cfg(feature = "execution")]
use rsscript_semantics::FrontendInputSnapshot;
use rsscript_semantics::{
    CompilationSession, analyze_source_with_interfaces,
    analyze_source_with_interfaces_without_core, analyze_source_without_core,
    standard_package_interfaces,
};

use super::{InterfaceSource, read_cli_source, read_interface_sources};

/// Which interfaces a command implies before the caller's explicit ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum InterfacePrelude {
    /// Core plus the language's standard package interfaces. This is what every
    /// command that compiles or checks ordinary RSScript uses.
    #[default]
    StandardPackages,
    /// `rss check --no-core`: the caller's explicit interfaces and nothing
    /// else. This is a single-file checking mode, not a build mode.
    None,
}

/// One single-file CLI input: the source the command named, the `--interface`
/// files it named, and the prelude its mode selects.
pub(crate) struct SourceInput {
    path: String,
    source: String,
    interfaces: Vec<InterfaceSource>,
    prelude: InterfacePrelude,
}

impl SourceInput {
    /// Read the source and every explicit interface through the same bounded,
    /// non-symlink CLI file reader.
    pub(crate) fn read(
        path: &str,
        interfaces: &[&str],
        prelude: InterfacePrelude,
    ) -> Result<Self, String> {
        let source = read_cli_source(Path::new(path))?;
        let interfaces = read_interface_sources(interfaces)?;
        Ok(Self {
            path: path.to_string(),
            source,
            interfaces,
            prelude,
        })
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    /// The assembled interface set: the prelude this mode implies followed by
    /// every `--interface` file, in the order the caller gave them.
    pub(crate) fn interfaces(&self) -> Vec<(&str, &str)> {
        let mut assembled = match self.prelude {
            InterfacePrelude::StandardPackages => standard_package_interfaces().to_vec(),
            InterfacePrelude::None => Vec::new(),
        };
        assembled.extend(
            self.interfaces
                .iter()
                .map(|interface| (interface.path.as_str(), interface.contents.as_str())),
        );
        assembled
    }

    /// The immutable frontend snapshot the compiling commands build from. It
    /// carries exactly the interfaces [`Self::analyze`] checks against.
    #[cfg(feature = "execution")]
    pub(crate) fn snapshot(&self) -> FrontendInputSnapshot {
        debug_assert_eq!(
            self.prelude,
            InterfacePrelude::StandardPackages,
            "compiling commands have no no-core mode"
        );
        FrontendInputSnapshot::from_sources(
            [(self.path.as_str(), self.source.as_str())],
            self.interfaces(),
        )
    }

    /// Analyze this input through the semantic-owned session query — the same
    /// query `Compiler::compile_snapshot` validates the snapshot with.
    pub(crate) fn analyze(&self) -> Vec<Diagnostic> {
        let interfaces = self.interfaces();
        // Session files have stable path identities. Preserve the legacy
        // analyzer's duplicate-interface diagnostics rather than silently
        // replacing one input buffer when a caller supplied the same logical
        // interface path twice.
        let unique_paths = interfaces
            .iter()
            .map(|(path, _)| *path)
            .collect::<BTreeSet<_>>();
        if unique_paths.len() != interfaces.len() {
            return self.legacy_analysis(&interfaces);
        }

        let mut session = match self.prelude {
            InterfacePrelude::StandardPackages => CompilationSession::default(),
            InterfacePrelude::None => CompilationSession::without_core(),
        };
        session
            .set_file(&self.path, &self.source)
            .expect("CLI source path must be a valid session path");
        for (path, contents) in interfaces {
            session
                .set_interface(path, contents)
                .expect("CLI interface path must be a valid session path");
        }
        session.workspace_analysis().diagnostics().to_vec()
    }

    fn legacy_analysis(&self, interfaces: &[(&str, &str)]) -> Vec<Diagnostic> {
        match self.prelude {
            InterfacePrelude::StandardPackages => {
                analyze_source_with_interfaces(&self.path, &self.source, interfaces)
            }
            InterfacePrelude::None if interfaces.is_empty() => {
                analyze_source_without_core(&self.path, &self.source)
            }
            InterfacePrelude::None => {
                analyze_source_with_interfaces_without_core(&self.path, &self.source, interfaces)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn in_memory(
        path: &str,
        source: &str,
        interfaces: &[(&str, &str)],
        prelude: InterfacePrelude,
    ) -> Self {
        Self {
            path: path.to_string(),
            source: source.to_string(),
            interfaces: interfaces
                .iter()
                .map(|(path, contents)| InterfaceSource {
                    path: (*path).to_string(),
                    contents: (*contents).to_string(),
                })
                .collect(),
            prelude,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InterfacePrelude, SourceInput};

    #[test]
    fn default_core_check_uses_the_session_owned_workspace_analysis() {
        let input = SourceInput::in_memory(
            "main.rss",
            "fn main() -> Int { return Host.value() }",
            &[("host.rssi", "module Host\npub fn value() -> Int\n")],
            InterfacePrelude::StandardPackages,
        );
        let diagnostics = input.analyze();
        assert!(
            diagnostics.is_empty(),
            "session-owned analysis should retain explicit interface visibility: {diagnostics:#?}"
        );
    }

    #[test]
    fn duplicate_interface_paths_preserve_the_legacy_analysis_behavior() {
        let source = "fn main() -> Int { return Host.value() }";
        let interfaces = [
            ("host.rssi", "module Host\npub fn value() -> Int\n"),
            ("host.rssi", "module Host\npub fn value() -> String\n"),
        ];
        let input = SourceInput::in_memory(
            "main.rss",
            source,
            &interfaces,
            InterfacePrelude::StandardPackages,
        );
        let mut combined = rsscript_semantics::standard_package_interfaces().to_vec();
        combined.extend(interfaces);
        assert_eq!(
            input.analyze(),
            rsscript_semantics::analyze_source_with_interfaces("main.rss", source, &combined)
        );
    }

    #[test]
    fn no_core_check_uses_the_session_owned_workspace_analysis() {
        let input = SourceInput::in_memory(
            "main.rss",
            "fn main() -> Int { return Host.value() }",
            &[("host.rssi", "module Host\npub fn value() -> Int\n")],
            InterfacePrelude::None,
        );
        let diagnostics = input.analyze();
        assert!(
            diagnostics.is_empty(),
            "session-owned no-core analysis should retain explicit interfaces: {diagnostics:#?}"
        );
    }

    #[test]
    fn no_core_duplicate_interfaces_preserve_legacy_analysis_behavior() {
        let source = "fn main() -> Int { return Host.value() }";
        let interfaces = [
            ("host.rssi", "module Host\npub fn value() -> Int\n"),
            ("host.rssi", "module Host\npub fn value() -> String\n"),
        ];
        let input = SourceInput::in_memory("main.rss", source, &interfaces, InterfacePrelude::None);
        assert_eq!(
            input.analyze(),
            rsscript_semantics::analyze_source_with_interfaces_without_core(
                "main.rss",
                source,
                &interfaces,
            ),
        );
    }

    /// The assembly `check` uses and the assembly the compiling commands use is
    /// the same list: this is the invariant that broke when `build` attached no
    /// standard package interfaces at all.
    #[cfg(feature = "execution")]
    #[test]
    fn the_snapshot_carries_exactly_the_analyzed_interfaces() {
        let input = SourceInput::in_memory(
            "main.rss",
            "fn main() -> Unit { return Unit }",
            &[("host.rssi", "module Host\npub fn value() -> Int\n")],
            InterfacePrelude::StandardPackages,
        );
        let assembled = input.interfaces();
        let snapshot = input.snapshot();
        let carried = snapshot
            .interfaces()
            .files()
            .iter()
            .map(|file| (file.path().to_string(), file.text().to_string()))
            .collect::<Vec<_>>();
        assert_eq!(
            carried,
            assembled
                .iter()
                .map(|(path, contents)| ((*path).to_string(), (*contents).to_string()))
                .collect::<Vec<_>>()
        );
        assert!(
            assembled.len() > 1,
            "the standard package prelude must be part of the assembly"
        );
    }
}
