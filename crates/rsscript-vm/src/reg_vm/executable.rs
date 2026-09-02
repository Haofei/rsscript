//! `RegVmExecutable` method implementation, split from `mod.rs` for module-size
//! partitioning (a second impl block; the type stays in `mod.rs`).

use super::*;
// `jit_missed_opt_report` lives in the native_stats_impl module; import it under
// the exact cfg of its sole caller (the diagnostics reporter below) so the import
// is present precisely when used.
#[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
use super::native_stats_impl::jit_missed_opt_report;

impl RegVmExecutable {
    /// Serialize this already-verified executable as `rsscript.bytecode.v1`.
    pub fn to_bytecode(&self) -> Result<Vec<u8>, EvalError> {
        self.artifact
            .to_bytes()
            .map_err(|error| EvalError::Runtime(error.to_string()))
    }

    pub fn bytecode_artifact(&self) -> &rsscript_bytecode::BytecodeArtifact {
        &self.artifact
    }

    pub fn typed_executable_facts(
        &self,
    ) -> Option<&rsscript_bytecode::BoundTypedExecutableFactsV1> {
        self.typed_executable_facts.as_deref()
    }

    /// Return the canonical result value for `main` when its v1 declaration
    /// contains enough structural type information to do so. Unsupported
    /// values return `None`; the report never fabricates dynamic identities.
    pub(in crate::reg_vm) fn main_result_wire_value(&self, value: VmValue) -> Option<WireValue> {
        let result = self
            .unit
            .native_signatures
            .get("main")?
            .return_type
            .as_deref()
            .map(WireType::parse)?;
        let record_layouts = self
            .unit
            .types
            .values()
            .map(|layout| WireRecordLayout {
                ty: WireType::parse(&layout.name),
                fields: layout
                    .fields
                    .iter()
                    .map(|field| WireRecordFieldLayout {
                        name: field.name.clone(),
                        ty: WireType::parse(&field.type_name),
                    })
                    .collect(),
            })
            .collect();
        let variant_layouts = self
            .unit
            .variant_layouts
            .values()
            .map(|layout| WireVariantLayout {
                ty: WireType::parse(&layout.name),
                variants: layout
                    .variants
                    .iter()
                    .map(|variant| WireVariantCaseLayout {
                        name: variant.name.clone(),
                        fields: variant
                            .fields
                            .iter()
                            .map(|field| WireRecordFieldLayout {
                                name: field.name.clone(),
                                ty: WireType::parse(&field.type_name),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        let signature = FunctionSignature {
            parameters: Vec::new(),
            result: result.clone(),
            asynchronous: false,
        };
        let types = WireCallTypeTable::for_signature(&signature)
            .and_then(|types| types.with_record_layouts(record_layouts))
            .and_then(|types| types.with_variant_layouts(variant_layouts))
            .ok()?;
        wire_value_from_vm_value(value, &result, &types).ok()
    }

    pub(in crate::reg_vm) fn prepare_vm(
        &self,
        args: Vec<String>,
        external_bindings: HashMap<String, ExternalFunction>,
        plan: &ExecutionPlan,
    ) -> Result<RegVm, EvalError> {
        let mut vm = RegVm::new(Rc::clone(&self.unit), args, external_bindings);
        vm.set_limits(plan.limits.clone());
        vm.stream_stdout = plan.stdout == StdoutMode::Streaming;
        match &plan.tier {
            TierPlan::Interpreter => {}
            TierPlan::Tier0 { force_all } => {
                vm.jit_enabled = true;
                vm.jit_force_all = *force_all;
            }
            #[cfg(feature = "native-jit")]
            TierPlan::Native(native) => {
                let verified_facts = match self.verified_facts.get_or_init(|| {
                    self.typed_executable_facts
                        .as_deref()
                        .map_or_else(
                            || VerifiedExecutableFacts::derive(&self.unit),
                            |typed| {
                                match VerifiedExecutableFacts::derive_with_typed(&self.unit, typed)
                                {
                                    Ok(facts) => Ok(facts),
                                    // Typed facts are optional optimization evidence. A
                                    // producer bug or hostile contradiction must never make
                                    // interpreter-correct execution fail merely because the
                                    // host opted into native acceleration. Discard all persisted
                                    // evidence and retain the bytecode-local conservative proof.
                                    Err(VerifiedFactsError::TypedFactsMismatch) => {
                                        VerifiedExecutableFacts::derive(&self.unit)
                                    }
                                    Err(error) => Err(error),
                                }
                            },
                        )
                        .map(Rc::new)
                }) {
                    Ok(facts) => Rc::clone(facts),
                    Err(error) => {
                        return Err(EvalError::Runtime(format!(
                            "cannot derive bounded verified executable facts: {error:?}"
                        )));
                    }
                };
                let mut native_state = NativeState::new_with_plan(native)?;
                let facts_summary = verified_facts.summary();
                native_state.stats.verified_known_reg_types = facts_summary.known_reg_types;
                native_state.stats.verified_unknown_reg_types = facts_summary.unknown_reg_types;
                native_state.stats.verified_known_call_sites = facts_summary.known_call_sites;
                native_state.stats.verified_instruction_effects = facts_summary.instruction_effects;
                native_state.verified_facts = Some(verified_facts);
                vm.native = Some(native_state);
                vm.jit_enabled = true;
                vm.jit_force_all = true;
            }
        }
        Ok(vm)
    }

    /// Per-function JIT eligibility analysis (the tier-0 "compile" step). A
    /// function is eligible when it is non-suspending and non-recursive — every
    /// instruction in the JIT-supported subset or a `CallKnown` to another
    /// eligible function (see [`compute_jit_eligibility`]); otherwise it falls
    /// back to the interpreter.
    pub fn jit_plan(&self) -> JitPlan {
        let mut plan = JitPlan::default();
        for function in &self.unit.functions {
            plan.total_functions += 1;
            let eligible = function.code.iter().all(jit_supported_instruction);
            if eligible {
                plan.eligible_functions += 1;
            } else {
                plan.fallback_functions += 1;
            }
        }
        plan
    }

    pub fn eval_main_with_args(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_external_bindings(
            args,
            std::iter::empty::<(String, ExternalFunction)>(),
        )
    }

    /// Run `main` with the tier-0 JIT enabled: JIT-eligible functions execute via
    /// the specializing executor, the rest via the interpreter. Output is
    /// identical to `eval_main_with_args` (verified by the N-way differential).
    pub fn eval_main_with_args_jit(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        // The differential/parity callers want every supported function JIT'd so
        // the whole covered subset is verified, not just loop functions.
        self.eval_main_with_args_and_external_bindings_jit_inner(
            args,
            std::iter::empty::<(String, ExternalFunction)>(),
            true,
            VmLimits::default(),
        )
    }

    /// Run the tier-0 executor with explicit limits. Trusted hosts that require
    /// unrestricted native-style recursion must opt in with
    /// [`VmLimits::unbounded_for_trusted_host`].
    pub fn eval_main_with_args_jit_and_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        limits: VmLimits,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_external_bindings_jit_inner(
            args,
            std::iter::empty::<(String, ExternalFunction)>(),
            true,
            limits,
        )
    }

    /// Run `main` with the native (Cranelift) JIT enabled: the integer/control
    /// core executes as machine code, tier-0 covers the rest of the supported
    /// subset, and the interpreter the remainder. Output is identical to
    /// `eval_main_with_args` (verified by the N-way differential). Compiles
    /// eligible functions on first call (threshold 0) so the differential
    /// exercises them.
    ///
    /// The default tier-up threshold is 0 (compile on first call), which keeps
    /// the differential's full coverage. Production hosts that want to defer
    /// compilation use [`NativeJitOptions::tier_up_threshold`]; the VM never
    /// reads process-global environment variables.
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        // Heap-aware precise resume is the diagnostic default. The force-deopt
        // entry point below explicitly opts out to cover replay-from-entry.
        self.eval_main_with_args_native_inner(args, NativeDiagnosticOptions::default())
            .map(|(output, _stats)| output)
    }

    /// Like [`Self::eval_main_with_args_native`] but also returns the native-tier
    /// [`NativeStats`] from the run (for benchmark/telemetry reporting).
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native_with_stats(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            NativeDiagnosticOptions {
                collect_stats: true,
                ..NativeDiagnosticOptions::default()
            },
        )
    }

    /// Benchmark/test entry point that pins tier-up without mutating process
    /// environment. Keeping this explicit prevents parallel test cases from
    /// changing one another's native compilation policy.
    #[doc(hidden)]
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native_at_threshold(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        tier_up_threshold: u32,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            NativeDiagnosticOptions {
                tier_up_threshold,
                ..NativeDiagnosticOptions::default()
            },
        )
        .map(|(output, _stats)| output)
    }

    /// Stats variant of [`Self::eval_main_with_args_native_at_threshold`].
    #[doc(hidden)]
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native_with_stats_at_threshold(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        tier_up_threshold: u32,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            NativeDiagnosticOptions {
                tier_up_threshold,
                collect_stats: true,
                ..NativeDiagnosticOptions::default()
            },
        )
    }

    /// Like [`Self::eval_main_with_args_native_osr`] but also returns the
    /// native-tier [`NativeStats`] (notably `osr_entries`) for bench telemetry.
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native_osr_with_stats(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            NativeDiagnosticOptions {
                collect_stats: true,
                eager_osr: true,
                ..NativeDiagnosticOptions::default()
            },
        )
    }

    /// Run `main` with the native tier AND OSR forced on (deterministically,
    /// independent of production options): a function with a qualifying native-subset
    /// hot loop runs that loop natively mid-function (OSR-entry at the header,
    /// OSR-exit/precise-resume at the post-loop ip). Must equal every other backend
    /// byte-for-byte. Test/validation + bench entry point.
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native_osr(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            NativeDiagnosticOptions {
                eager_osr: true,
                ..NativeDiagnosticOptions::default()
            },
        )
        .map(|(output, _stats)| output)
    }

    /// Run `main` with the native tier AND precise resume forced on,
    /// regardless of the production plan. Native code runs for real; when it
    /// bails at a real guard safepoint, the live interpreter register window is
    /// reconstructed and interpretation resumes AT the safepoint (instead of re-
    /// running the function from the top). The observable result must equal every
    /// non-precise backend. Test/validation entry point only — lets the test set
    /// `precise_deopt` deterministically without a (racy) process env var.
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native_precise(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(args, NativeDiagnosticOptions::default())
            .map(|(output, _stats)| output)
    }

    /// Run `main` with the native tier in **deopt stress mode**: the native code
    /// always bails at its first guard, so every native-eligible function falls
    /// back to the interpreter. Its output must equal every other backend — this
    /// is how the deopt/fallback path is verified end-to-end.
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native_force_deopt(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            NativeDiagnosticOptions {
                force_bail: true,
                precise_deopt: false,
                ..NativeDiagnosticOptions::default()
            },
        )
        .map(|(output, _stats)| output)
    }

    /// Run `main` while forcing the selected native safepoint to deopt. Unlike
    /// [`Self::eval_main_with_args_native_force_deopt`], this still enters native
    /// code and captures the safepoint's live register payload before falling back
    /// or precise-resuming, so it exercises the real deopt machinery.
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native_force_safepoint(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        safepoint: u32,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            NativeDiagnosticOptions {
                forced_safepoint: Some(safepoint),
                ..NativeDiagnosticOptions::default()
            },
        )
        .map(|(output, _stats)| output)
    }

    /// Run `main` while forcing every generated native safepoint to deopt.
    /// This explicit switch is deterministic and safe
    /// for in-process differential tests and fuzzers.
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native_force_all_safepoints(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            NativeDiagnosticOptions {
                force_all_safepoints: true,
                ..NativeDiagnosticOptions::default()
            },
        )
        .map(|(output, _stats)| output)
    }

    /// Test/validation entry point for the lever-2 missed-optimization report. Runs
    /// `main` with the native tier + OSR forced on AND the report armed deterministically
    /// returning the report block lines
    /// alongside the stats. The report is observational, so the `EvalOutput` is byte-
    /// identical to [`Self::eval_main_with_args_native_osr`]; this just also hands the
    /// caller the report so a test can assert the per-region reasons.
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native_osr_report(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<(EvalOutput, NativeStats, Vec<String>), EvalError> {
        // Cranelift does not yet account the interpreter's execution budgets.
        // This explicitly native, experimental entry point is therefore trusted-only;
        // bounded callers must use the corresponding `_with_limits` entry point,
        // which keeps execution on the interpreter while limits are armed.
        self.eval_main_with_args_native_inner_reported(
            args,
            NativeDiagnosticOptions {
                collect_stats: true,
                eager_osr: true,
                report: true,
                ..NativeDiagnosticOptions::default()
            },
            VmLimits::unbounded_for_trusted_host(),
        )
    }

    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub(in crate::reg_vm) fn eval_main_with_args_native_inner(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        diagnostics: NativeDiagnosticOptions,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        // Native code does not yet poll or account interpreter budgets. Keep this
        // opt-in experimental API explicit about its trusted-host execution model.
        self.eval_main_with_args_native_inner_reported(
            args,
            diagnostics,
            VmLimits::unbounded_for_trusted_host(),
        )
        .map(|(output, stats, _lines)| (output, stats))
    }

    /// Like [`Self::eval_main_with_args_native_with_stats`] but runs under explicit
    /// [`VmLimits`]. With native enabled, an armed `step_budget`/`cancel`/`allocation_budget`
    /// must prevent native dispatch (Cranelift polls/accounts none of them) — used to
    /// regression-test the recursive native fast-path limit gate.
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native_with_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        limits: VmLimits,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        self.eval_main_with_args_native_inner_reported(
            args,
            NativeDiagnosticOptions {
                collect_stats: true,
                ..NativeDiagnosticOptions::default()
            },
            limits,
        )
        .map(|(output, stats, _lines)| (output, stats))
    }

    /// Like [`Self::eval_main_with_args_native_osr_with_stats`] but under explicit
    /// [`VmLimits`]. Any armed step, cancellation, memory, or host-call limit keeps
    /// OSR on the interpreter until transformed regions carry exact source resource
    /// costs. Tests use this entry point to verify both the result and that
    /// `osr_entries == 0` under those modes.
    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub fn eval_main_with_args_native_osr_with_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        limits: VmLimits,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        self.eval_main_with_args_native_inner_reported(
            args,
            NativeDiagnosticOptions {
                collect_stats: true,
                eager_osr: true,
                ..NativeDiagnosticOptions::default()
            },
            limits,
        )
        .map(|(output, stats, _lines)| (output, stats))
    }

    #[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
    pub(in crate::reg_vm) fn eval_main_with_args_native_inner_reported(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        diagnostics: NativeDiagnosticOptions,
        limits: VmLimits,
    ) -> Result<(EvalOutput, NativeStats, Vec<String>), EvalError> {
        let native_plan = NativeExecutionPlan::for_diagnostics(diagnostics);
        let plan = ExecutionPlan::native(limits, native_plan);
        let mut vm = self.prepare_vm(
            args.into_iter().map(Into::into).collect(),
            HashMap::new(),
            &plan,
        )?;
        let value = vm.run_program("main")?;
        let jit_state = &vm.jit_state;
        if let Some(native) = &mut vm.native
            && native.collect_stats
        {
            native.stats.add_profile_feedback(&self.unit, jit_state);
            let loop_evidence = self
                .loop_optimization_evidence
                .get_or_init(|| unit_loop_optimization_evidence(&self.unit));
            native.stats.add_loop_optimization_evidence(loop_evidence);
            let facts = native
                .verified_facts
                .as_deref()
                .expect("native execution installs verified facts");
            native
                .stats
                .add_native_decline_reasons(&self.unit, jit_state, facts);
        }
        // Diagnostics are returned as structured values. Process-global logging
        // belongs to the CLI/composition root, not the embeddable VM.
        let report_lines = if let Some(native) = &vm.native
            && native.report
        {
            jit_missed_opt_report(&self.unit, &vm.jit_state, native)
        } else {
            Vec::new()
        };
        let stats = vm
            .native
            .as_ref()
            .map(|native| native.stats.clone())
            .unwrap_or_default();
        vm.cleanup_provider_resources()?;
        let display_value = value.display();
        Ok((
            EvalOutput {
                usage: vm.usage(),
                value: display_value.clone(),
                display_value,
                stdout: vm.stdout,
                stderr: vm.stderr,
                provider_call_traces: vm.provider_trace.snapshot(),
            },
            stats,
            report_lines,
        ))
    }

    /// Like [`eval_main_with_args_jit`] but with native host bindings, using the
    /// production has-loop heuristic (only loop functions are JIT'd).
    pub fn eval_main_with_args_and_external_bindings_jit(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_external_bindings_jit_inner(
            args,
            external_bindings,
            false,
            VmLimits::default(),
        )
    }

    pub(in crate::reg_vm) fn eval_main_with_args_and_external_bindings_jit_inner(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        force_all: bool,
        limits: VmLimits,
    ) -> Result<EvalOutput, EvalError> {
        let plan = ExecutionPlan::tier0(limits, force_all);
        let mut vm = self.prepare_vm(
            args.into_iter().map(Into::into).collect(),
            external_bindings
                .into_iter()
                .map(|(key, function)| (key.into(), function))
                .collect(),
            &plan,
        )?;
        let value = vm.run_program("main")?;
        vm.cleanup_provider_resources()?;
        let display_value = value.display();
        Ok(EvalOutput {
            usage: vm.usage(),
            value: display_value.clone(),
            display_value,
            stdout: vm.stdout,
            stderr: vm.stderr,
            provider_call_traces: vm.provider_trace.snapshot(),
        })
    }

    /// Like [`Self::eval_main_with_args_and_external_bindings`] but streams program
    /// stdout (`Output.write` output) live to the real process stdout, line-flushed,
    /// as the program runs. This lets a library caller show output immediately
    /// instead of buffering until exit. The returned
    /// `EvalOutput.stdout` is still the full captured buffer (identical to the
    /// non-streaming call), so the program output has already been written to the
    /// terminal — the caller must NOT print it a second time.
    pub fn eval_main_with_args_and_external_bindings_streaming_stdout(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_external_bindings_streaming_stdout_with_limits(
            args,
            external_bindings,
            VmLimits::default(),
        )
    }

    /// Streaming variant with an explicit output/resource budget.
    pub fn eval_main_with_args_and_external_bindings_streaming_stdout_with_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        limits: VmLimits,
    ) -> Result<EvalOutput, EvalError> {
        let plan = ExecutionPlan::streaming(limits);
        let mut vm = self.prepare_vm(
            args.into_iter().map(Into::into).collect(),
            external_bindings
                .into_iter()
                .map(|(key, function)| (key.into(), function))
                .collect(),
            &plan,
        )?;
        let result = vm.run_program("main");
        // Flush any final line that lacks a trailing newline so no output is lost.
        let flush_result = if vm.stream_flushed < vm.stdout.len() {
            let mut out = std::io::stdout();
            out.write_all(&vm.stdout.as_bytes()[vm.stream_flushed..])
                .and_then(|()| out.flush())
                .map_err(|error| EvalError::Runtime(format!("failed to stream stdout: {error}")))
        } else {
            Ok(())
        };
        let value = result?;
        flush_result?;
        vm.cleanup_provider_resources()?;
        let display_value = value.display();
        Ok(EvalOutput {
            usage: vm.usage(),
            value: display_value.clone(),
            display_value,
            stdout: vm.stdout,
            stderr: vm.stderr,
            provider_call_traces: vm.provider_trace.snapshot(),
        })
    }

    /// Run `main` under explicit resource limits ([`VmLimits`]). Limits convert
    /// selected runaway behavior into `EvalError::Runtime`; they are not an
    /// isolation boundary. Output is otherwise identical to
    /// [`Self::eval_main_with_args`].
    pub fn eval_main_with_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        limits: VmLimits,
    ) -> Result<EvalOutput, EvalError> {
        let plan = ExecutionPlan::interpreter(limits);
        let mut vm = self.prepare_vm(
            args.into_iter().map(Into::into).collect(),
            HashMap::new(),
            &plan,
        )?;
        let value = vm.run_program("main")?;
        vm.cleanup_provider_resources()?;
        let display_value = value.display();
        Ok(EvalOutput {
            usage: vm.usage(),
            value: display_value.clone(),
            display_value,
            stdout: vm.stdout,
            stderr: vm.stderr,
            provider_call_traces: vm.provider_trace.snapshot(),
        })
    }

    pub fn eval_main_with_args_and_external_bindings(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_external_bindings_and_limits(
            args,
            external_bindings,
            VmLimits::default(),
        )
    }

    /// Execute under explicit limits while retaining partial usage, output,
    /// Provider traces, and cleanup counters on failure.
    pub fn execute_main_with_args_and_external_bindings_and_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        limits: VmLimits,
    ) -> Result<EvalExecutionReport, EvalError> {
        let plan = ExecutionPlan::interpreter(limits);
        self.execute_main_with_args_and_external_bindings_plan(args, external_bindings, plan)
    }

    /// Execute through the adaptive native tier while preserving the same
    /// verified bytecode, Provider linking, limits, cleanup, and report path as
    /// the reference interpreter. Unsupported or unprofitable regions fall back
    /// to the interpreter. Armed limits remain authoritative: whole-function
    /// native dispatch is refused where exact accounting is unavailable, while
    /// verified OSR regions use the native limits cells.
    #[cfg(feature = "native-jit")]
    pub fn execute_main_with_args_and_external_bindings_native_and_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        limits: VmLimits,
    ) -> Result<EvalExecutionReport, EvalError> {
        self.execute_main_with_args_and_external_bindings_native_options_and_limits(
            args,
            external_bindings,
            NativeJitOptions::default(),
            limits,
        )
    }

    /// Native execution with a deterministic host-owned policy. This is the
    /// production embedding entry point; it never reads process environment.
    #[cfg(feature = "native-jit")]
    pub fn execute_main_with_args_and_external_bindings_native_options_and_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        options: NativeJitOptions,
        limits: VmLimits,
    ) -> Result<EvalExecutionReport, EvalError> {
        let native = NativeExecutionPlan::from_options(options);
        let plan = ExecutionPlan::native(limits, native);
        self.execute_main_with_args_and_external_bindings_plan(args, external_bindings, plan)
    }

    pub(in crate::reg_vm) fn execute_main_with_args_and_external_bindings_plan(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        plan: ExecutionPlan,
    ) -> Result<EvalExecutionReport, EvalError> {
        let mut vm = self.prepare_vm(
            args.into_iter().map(Into::into).collect(),
            external_bindings
                .into_iter()
                .map(|(key, function)| (key.into(), function))
                .collect(),
            &plan,
        )?;
        let execution = vm.run_program("main");
        #[cfg(feature = "native-jit")]
        if let Some(native) = &mut vm.native
            && native.collect_stats
        {
            let phases = native.compile_phase_timings();
            native.stats.validation_nanos = phases.validation_nanos;
            native.stats.codegen_nanos = phases.codegen_nanos;
            native.stats.finalize_nanos = phases.finalize_nanos;
            native.stats.add_profile_feedback(&self.unit, &vm.jit_state);
            let loop_evidence = self
                .loop_optimization_evidence
                .get_or_init(|| unit_loop_optimization_evidence(&self.unit));
            native.stats.add_loop_optimization_evidence(loop_evidence);
            let facts = native
                .verified_facts
                .as_deref()
                .expect("native execution installs verified facts");
            native
                .stats
                .add_native_decline_reasons(&self.unit, &vm.jit_state, facts);
        }
        #[cfg(feature = "native-jit")]
        let engine =
            vm.native
                .as_ref()
                .map_or(crate::ExecutionEngineTelemetry::Interpreter, |native| {
                    crate::ExecutionEngineTelemetry::Native(Box::new(
                        crate::NativeExecutionEngineTelemetry {
                            considered: native.stats.considered,
                            compiled: native.stats.compiled,
                            baseline_compiles: native.stats.baseline_compiles,
                            optimized_compiles: native.stats.optimized_compiles,
                            baseline_calls: native.stats.baseline_calls,
                            optimized_calls: native.stats.optimized_calls,
                            native_calls: native.stats.native_calls,
                            native_bails: native.stats.native_bails,
                            osr_entries: native.stats.osr_entries,
                            continuation_entries: native.stats.continuation_entries,
                            continuation_candidate_checks: native
                                .stats
                                .continuation_candidate_checks,
                            continuation_full_probes: native.stats.continuation_full_probes,
                            continuation_instance_key_builds: native
                                .stats
                                .continuation_instance_key_builds,
                            continuation_yields: native.stats.continuation_yields,
                            continuation_compiled_source_instructions: native
                                .stats
                                .continuation_compiled_source_instructions,
                            interpreted_native_work: native.stats.interpreted_native_work,
                            native_barrier_counts: Box::new(
                                native.stats.native_barrier_counts.clone(),
                            ),
                            direct_list_bounds_check_sites: native
                                .stats
                                .direct_list_bounds_check_sites,
                            direct_list_bounds_checks_elided: native
                                .stats
                                .direct_list_bounds_checks_elided,
                            readonly_licm_sites: native.stats.memoized_runtime_helper_call_sites,
                            runtime_helper_call_sites: native.stats.runtime_helper_call_sites,
                            resident_code_bytes: native.stats.compiled_code_bytes,
                            published_code_bytes: native.admission.admitted_code_bytes,
                            rejected_resident_bytes: 0,
                            reserved_arena_bytes: native.executable_memory_budget.allocated(),
                            translation_nanos: native.stats.translation_nanos,
                            validation_nanos: native.stats.validation_nanos,
                            codegen_nanos: native.stats.codegen_nanos,
                            finalize_nanos: native.stats.finalize_nanos,
                            compile_nanos: native.stats.compile_nanos,
                            run_nanos: native.stats.run_nanos,
                        },
                    ))
                });
        #[cfg(not(feature = "native-jit"))]
        let engine = crate::ExecutionEngineTelemetry::Interpreter;
        let cleanup = vm.cleanup_provider_resources();
        let result = match (execution, cleanup) {
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        };
        let usage = vm.usage();
        let provider_call_traces = vm.provider_trace.snapshot();
        let stdout = vm.stdout;
        let stderr = vm.stderr;
        match result {
            Ok(value) => {
                let display_value = value.display();
                let wire_value = self.main_result_wire_value(value.clone());
                Ok(EvalExecutionReport {
                    usage,
                    value: Some(display_value.clone()),
                    display_value: Some(display_value),
                    wire_value,
                    stdout,
                    stderr,
                    provider_call_traces,
                    engine: engine.clone(),
                    failure: None,
                })
            }
            Err(error) => Ok(EvalExecutionReport {
                usage,
                value: None,
                display_value: None,
                wire_value: None,
                stdout,
                stderr,
                provider_call_traces,
                engine,
                failure: Some(error),
            }),
        }
    }

    pub fn eval_main_with_args_and_external_bindings_and_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        limits: VmLimits,
    ) -> Result<EvalOutput, EvalError> {
        let plan = ExecutionPlan::interpreter(limits);
        let mut vm = self.prepare_vm(
            args.into_iter().map(Into::into).collect(),
            external_bindings
                .into_iter()
                .map(|(key, function)| (key.into(), function))
                .collect(),
            &plan,
        )?;
        let value = vm.run_program("main")?;
        vm.cleanup_provider_resources()?;
        let display_value = value.display();
        Ok(EvalOutput {
            usage: vm.usage(),
            value: display_value.clone(),
            display_value,
            stdout: vm.stdout,
            stderr: vm.stderr,
            provider_call_traces: vm.provider_trace.snapshot(),
        })
    }
}
