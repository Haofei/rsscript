use super::*;

impl NativeModule {
    fn decode_deopt_live(site: &DeoptSite, payload_base: usize, payload: &[i64]) -> Vec<DeoptReg> {
        site.live
            .iter()
            .filter_map(|&(reg, ty)| {
                let &bits = payload.get(payload_base + reg as usize)?;
                // Handles are table indexes and can be reconstructed. Flat
                // registers are raw borrow-pinned pointers, so the interpreter
                // frame must retain them rather than decoding them as scalars.
                let value = match ty {
                    JitValueType::Int => DeoptValue::Int(bits),
                    JitValueType::Bool => DeoptValue::Bool(bits != 0),
                    JitValueType::Float => DeoptValue::Float(f64::from_bits(bits as u64)),
                    JitValueType::Handle => DeoptValue::Handle(bits),
                    JitValueType::FlatInt
                    | JitValueType::FlatIntMut
                    | JitValueType::FlatFloat
                    | JitValueType::FlatFloatMut => return None,
                };
                Some(DeoptReg { reg, value })
            })
            .collect()
    }

    fn decode_deopt_child(
        &self,
        child: DeoptChildSite,
        payload_base: usize,
        payload: &[i64],
    ) -> Option<Box<DeoptFrame>> {
        let safepoint_bits = *payload.get(payload_base + child.safepoint_slot as usize)?;
        if safepoint_bits <= 0 {
            return None;
        }
        self.decode_deopt_frame(
            child.callee,
            SafepointId(safepoint_bits as u32),
            payload_base + child.payload_slot as usize,
            payload,
        )
        .map(Box::new)
    }

    fn decode_deopt_frame(
        &self,
        function: CompiledId,
        safepoint_id: SafepointId,
        payload_base: usize,
        payload: &[i64],
    ) -> Option<DeoptFrame> {
        if function.module_id != self.id || safepoint_id.0 == 0 {
            return None;
        }
        let func = self.funcs.get(function.index)?;
        let site = func.deopt_map.sites.get(safepoint_id.0 as usize - 1)?;
        let live = Self::decode_deopt_live(site, payload_base, payload);
        let child = site
            .child
            .and_then(|child| self.decode_deopt_child(child, payload_base, payload));
        Some(DeoptFrame {
            function,
            safepoint_id,
            live,
            child,
        })
    }

    pub(super) fn call_inner(
        &self,
        session: &mut NativeCallSession,
        id: CompiledId,
        invocation: NativeCallInvocation<'_>,
    ) -> NativeOutcome {
        let NativeCallInvocation {
            args,
            lens,
            host_ctx,
            logical_depth,
            limits_ptr,
        } = invocation;
        if id.module_id != self.id {
            return NativeOutcome::Deopt {
                safepoint_id: SafepointId::ANONYMOUS,
                live: Vec::new(),
                child: None,
                logical_depth: None,
                decline: None,
            };
        }
        let func = match self.funcs.get(id.index) {
            Some(func) => func,
            None => {
                return NativeOutcome::Deopt {
                    safepoint_id: SafepointId::ANONYMOUS,
                    live: Vec::new(),
                    child: None,
                    logical_depth: None,
                    decline: None,
                };
            }
        };
        if func.requires_limits == limits_ptr.is_null()
            || func.native_call_depth > func.native_depth_cap
        {
            return anonymous_deopt();
        }
        let required = if func.osr { func.n_regs } else { func.n_params };
        if args.len() != required || lens.len() != required {
            return anonymous_deopt();
        }
        let Some(_call_guard) = TopLevelCallGuard::enter() else {
            return reentrant_decline();
        };
        let mut out = 0_i64;
        let mut bail = 0_u8;
        let mut safepoint = 0_i64;
        let payload_ptr = session.ensure_payload(func.deopt_map.payload_words);
        let mut helper_context = HostCallContext {
            user: host_ctx,
            bail: &mut bail,
        };
        let mut frame = JitCallFrame {
            abi_version: JIT_CALL_ABI_VERSION,
            frame_size: CALL_FRAME_SIZE,
            flags: 0,
            args: args.as_ptr(),
            lens: lens.as_ptr(),
            arg_count: args.len(),
            host_ctx: (&mut helper_context as *mut HostCallContext) as HostCtx,
            limits: limits_ptr,
            result: &mut out,
            bail: &mut bail,
            safepoint: &mut safepoint,
            deopt: payload_ptr,
            native_depth: 0,
            logical_depth: logical_depth.current,
            logical_depth_limit: logical_depth.limit,
        };
        // SAFETY: the entry was finalized with `CompiledAbi`; every frame
        // pointer and borrowed argument remains live for this activation.
        let completed = unsafe { (func.f)(&mut frame) };
        if completed == JitStatus::AbiMismatch {
            abi_mismatch_decline()
        } else if completed == JitStatus::Completed && bail == 0 {
            if func.returns_handle {
                NativeOutcome::CompletedHandle(out)
            } else {
                NativeOutcome::Completed(out)
            }
        } else if completed == JitStatus::Yielded && bail == 0 {
            NativeOutcome::Yield {
                exit_id: u32::try_from(out).unwrap_or(u32::MAX),
            }
        } else {
            let safepoint_id = SafepointId(safepoint as u32);
            let frame = self.decode_deopt_frame(id, safepoint_id, 0, &session.deopt_payload);
            NativeOutcome::Deopt {
                safepoint_id,
                live: frame
                    .as_ref()
                    .map_or_else(Vec::new, |frame| frame.live.clone()),
                child: frame.and_then(|frame| frame.child),
                logical_depth: func.osr.then_some(out.max(0) as usize),
                decline: None,
            }
        }
    }
}
