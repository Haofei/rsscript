#[test]
fn distinct_bail_sites_get_stable_safepoint_ids() {
    use JitValueType::{FlatInt, Int};
    let mut m = module();

    // fn(a: FlatInt, x: Int, i: Int) -> Int { t = x + x; return a[i] }
    // Two distinct bail sites: the `Add` overflow guard (site 1) precedes the
    // `ListGetIntDirect` OOB guard (site 2). regs 0=a,1=x,2=i,3=t,4=res.
    let id = m
        .compile(&ft(
            3,
            vec![FlatInt, Int, Int, Int, Int],
            vec![
                JitInstr::Add {
                    dst: 3,
                    lhs: 1,
                    rhs: 1,
                },
                JitInstr::ListGetIntDirect {
                    dst: 4,
                    base: 0,
                    index: 2,
                },
                JitInstr::Return { src: 4 },
            ],
        ))
        .unwrap();
    let ints: Vec<i64> = vec![10, 20, 30];
    let ints_ptr = ints.as_ptr() as i64;
    let ilen = ints.len() as i64;

    // Bail at the FIRST site: x + x overflows, so the `Add` guard fires (id 1)
    // before the list read is ever reached.
    assert!(matches!(
        m.call_with_host_ctx(
            id,
            &[ints_ptr, i64::MAX, 0],
            &[ilen, 0, 0],
            0,
            &mut [FlatBufferArg::Int(&ints)]
        ),
        NativeOutcome::Deopt {
            safepoint_id: SafepointId(1),
            ..
        }
    ));
    // Pass the first guard (small x, no overflow) but bail at the SECOND site:
    // index 5 is out of bounds, so the direct-read OOB guard fires (id 2).
    assert!(matches!(
        m.call_with_host_ctx(
            id,
            &[ints_ptr, 1, 5],
            &[ilen, 0, 0],
            0,
            &mut [FlatBufferArg::Int(&ints)]
        ),
        NativeOutcome::Deopt {
            safepoint_id: SafepointId(2),
            ..
        }
    ));
    // Both guards pass → completes (id stays 0 = no bail recorded).
    assert!(matches!(
        m.call_with_host_ctx(
            id,
            &[ints_ptr, 1, 2],
            &[ilen, 0, 0],
            0,
            &mut [FlatBufferArg::Int(&ints)]
        ),
        NativeOutcome::Completed(_)
    ));
}

// --- deopt state-map: deopt state-map (must-analysis) -------------------------------

#[test]
fn deopt_map_straightline_single_guard() {
    // fn(a, b) { t = a + b; return t }  regs 0=a,1=b,2=t. The `Add` (ip 0) has
    // one overflow guard (site id 1) → one site, resuming at ip 0 with the two
    // params live (t is not yet assigned on entry to its own instruction).
    let mut m = module();
    let id = m
        .compile(&f(
            2,
            3,
            vec![
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    let map = m.deopt_map(id).expect("map for valid id");
    assert_eq!(map.sites.len(), 1);
    assert_eq!(map.sites[0].resume_ip, 0);
    assert_eq!(
        map.sites[0].live,
        vec![(0, JitValueType::Int), (1, JitValueType::Int)]
    );
}

#[test]
fn profiled_branch_deopts_on_cold_edge() {
    let mut m = module();
    let id = m
        .compile(&f(
            2,
            3,
            vec![
                JitInstr::ProfiledJumpIfIntCompare {
                    lhs: 0,
                    rhs: 1,
                    op: JitCompare::Lt,
                    expected: true,
                    target: 3,
                    hot_target: false,
                },
                JitInstr::LoadInt { dst: 2, value: 10 },
                JitInstr::Return { src: 2 },
                JitInstr::LoadInt { dst: 2, value: 99 },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();

    assert_eq!(m.call(id, &[5, 3], &[0, 0]).completed(), Some(10));
    match m.call(id, &[1, 3], &[0, 0]) {
        NativeOutcome::Deopt {
            safepoint_id, live, ..
        } => {
            assert_eq!(safepoint_id, SafepointId(1));
            assert_eq!(
                live.iter().find(|reg| reg.reg == 0).map(|reg| reg.value),
                Some(DeoptValue::Int(1))
            );
            assert_eq!(
                live.iter().find(|reg| reg.reg == 1).map(|reg| reg.value),
                Some(DeoptValue::Int(3))
            );
        }
        other => panic!("expected profiled cold edge to deopt, got {other:?}"),
    }
}

#[test]
fn deopt_map_two_distinct_sites_track_prior_defs() {
    use JitValueType::{FlatInt, Int};
    // fn(a: FlatInt, x, i) { t = x + x; return a[i] }  regs 0=a,1=x,2=i,3=t,4=res.
    // Site 1: the `Add` overflow guard at ip 0 (t not yet live). Site 2: the
    // `ListGetIntDirect` OOB guard at ip 1 — by then `t` (reg 3) is definitely
    // assigned, so it appears in site 2's live set but not site 1's. (Mirrors
    // `distinct_bail_sites_get_stable_safepoint_ids`.)
    let mut m = module();
    let id = m
        .compile(&ft(
            3,
            vec![FlatInt, Int, Int, Int, Int],
            vec![
                JitInstr::Add {
                    dst: 3,
                    lhs: 1,
                    rhs: 1,
                },
                JitInstr::ListGetIntDirect {
                    dst: 4,
                    base: 0,
                    index: 2,
                },
                JitInstr::Return { src: 4 },
            ],
        ))
        .unwrap();
    let map = m.deopt_map(id).expect("map for valid id");
    assert_eq!(map.sites.len(), 2);

    // Site 1 (id 1): resume at the Add (ip 0); params live, t (reg 3) is NOT.
    assert_eq!(map.sites[0].resume_ip, 0);
    assert!(!map.sites[0].live.iter().any(|(r, _)| *r == 3));
    // Params 0..3 are live (a is FlatInt, x/i are Int).
    assert_eq!(
        map.sites[0].live,
        vec![
            (0, JitValueType::FlatInt),
            (1, JitValueType::Int),
            (2, JitValueType::Int)
        ]
    );

    // Site 2 (id 2): resume at the direct read (ip 1); t (reg 3) is now live.
    assert_eq!(map.sites[1].resume_ip, 1);
    assert!(map.sites[1].live.contains(&(3, JitValueType::Int)));
}

#[test]
fn deopt_map_must_analysis_excludes_one_armed_def() {
    // A register assigned on only ONE arm before a join with a guard must NOT be
    // in the join's live set (intersection / must-analysis).
    //
    //   0: if cond(reg1) goto 3            (cond is param reg 1)
    //   1:   t(reg3) = a(reg0) + a(reg0)   only the fall-through arm assigns t
    //   2:   goto 4
    //   3:   nop                           the taken arm leaves t unassigned
    //   4:   u(reg4) = a + a               guard here joins both arms
    //   5:   return u
    // regs: 0=a, 1=cond, 2=(unused scratch), 3=t, 4=u.
    use JitValueType::{Bool, Int};
    let mut m = module();
    let id = m
        .compile(&ft(
            2,
            vec![Int, Bool, Int, Int, Int],
            vec![
                JitInstr::JumpIfBool {
                    cond: 1,
                    expected: true,
                    target: 3,
                },
                JitInstr::Add {
                    dst: 3,
                    lhs: 0,
                    rhs: 0,
                },
                JitInstr::Jump { target: 4 },
                JitInstr::Nop,
                JitInstr::Add {
                    dst: 4,
                    lhs: 0,
                    rhs: 0,
                },
                JitInstr::Return { src: 4 },
            ],
        ))
        .unwrap();
    let map = m.deopt_map(id).expect("map for valid id");
    // Two Add guards: site 1 at ip 1, site 2 at the post-join ip 4.
    assert_eq!(map.sites.len(), 2);
    assert_eq!(map.sites[0].resume_ip, 1);
    assert_eq!(map.sites[1].resume_ip, 4);
    // The key assertion: at the post-join guard (ip 4), `t` (reg 3) is assigned
    // on only one arm, so intersection excludes it from the live set.
    assert!(
        !map.sites[1].live.iter().any(|(r, _)| *r == 3),
        "reg 3 assigned on only one arm must not be live at the join: {:?}",
        map.sites[1].live
    );
    // The params (regs 0 and 1) are assigned on every path → still live.
    assert!(map.sites[1].live.contains(&(0, JitValueType::Int)));
    assert!(map.sites[1].live.contains(&(1, JitValueType::Bool)));
    // On the fall-through arm's own guard (ip 1) t is also not-yet live.
    assert!(!map.sites[0].live.iter().any(|(r, _)| *r == 3));
}

#[test]
fn deopt_map_rejects_foreign_id() {
    // A foreign / out-of-range id yields no map, mirroring `call`'s validation.
    let mut m1 = module();
    let mut m2 = module();
    let id1 = m1.compile(&super::validation::two_param_add()).unwrap();
    let _id2 = m2.compile(&super::validation::two_param_add()).unwrap();
    assert!(m1.deopt_map(id1).is_some());
    assert!(m2.deopt_map(id1).is_none());
}

#[test]
fn compiles_and_runs_add() {
    let mut m = module();
    // fn(a, b) { return a + b }   regs: 0=a,1=b,2=tmp
    let id = m
        .compile(&f(
            2,
            3,
            vec![
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    assert_eq!(m.callt(id, &[3, 4]), Some(7));
    assert_eq!(m.callt(id, &[-10, 4]), Some(-6));
    // overflow bails:
    assert_eq!(m.callt(id, &[i64::MAX, 1]), None);
}

#[test]
fn loop_sum_to_n() {
    // fn(n) { total=0; i=1; while i<=n { total+=i; i+=1 } return total }
    // regs: 0=n, 1=total, 2=i, 3=one
    let mut m = module();
    let code = vec![
        JitInstr::LoadInt { dst: 1, value: 0 }, // 0 total=0
        JitInstr::LoadInt { dst: 2, value: 1 }, // 1 i=1
        JitInstr::LoadInt { dst: 3, value: 1 }, // 2 one=1
        // 3: loop head: if !(i<=n) goto end(8)
        JitInstr::JumpIfIntCompare {
            lhs: 2,
            rhs: 0,
            op: JitCompare::Le,
            expected: false,
            target: 8,
        },
        JitInstr::Add {
            dst: 1,
            lhs: 1,
            rhs: 2,
        }, // 4 total+=i
        JitInstr::Add {
            dst: 2,
            lhs: 2,
            rhs: 3,
        }, // 5 i+=1
        JitInstr::Jump { target: 3 }, // 6 loop
        JitInstr::Nop,                // 7 (padding leader)
        JitInstr::Return { src: 1 },  // 8 end
    ];
    let id = m.compile(&f(1, 4, code)).unwrap();
    assert_eq!(m.callt(id, &[10]), Some(55));
    assert_eq!(m.callt(id, &[0]), Some(0));
    assert_eq!(m.callt(id, &[100]), Some(5050));
}

#[test]
fn osr_entry_runs_loop_and_exits_with_live_out() {
    // Same loop as `loop_sum_to_n`, but compiled as an OSR-entry at the loop
    // header (ip 3) with the post-loop `Return` replaced by `OsrExit`. The
    // entry loads the live-in window (regs definitely-assigned at ip 3: total,
    // i, one, n) and jumps into the loop; the loop runs natively; on exit it
    // deopts at ip 8 with the live-out window (`total`).
    let mut m = module();
    let code = vec![
        JitInstr::LoadInt { dst: 1, value: 0 }, // 0 total=0 (pre-loop; not run under OSR)
        JitInstr::LoadInt { dst: 2, value: 1 }, // 1 i=1
        JitInstr::LoadInt { dst: 3, value: 1 }, // 2 one=1
        // 3: loop head: if !(i<=n) goto end(8)
        JitInstr::JumpIfIntCompare {
            lhs: 2,
            rhs: 0,
            op: JitCompare::Le,
            expected: false,
            target: 8,
        },
        JitInstr::Add {
            dst: 1,
            lhs: 1,
            rhs: 2,
        }, // 4 total+=i
        JitInstr::Add {
            dst: 2,
            lhs: 2,
            rhs: 3,
        }, // 5 i+=1
        JitInstr::Jump { target: 3 }, // 6 loop
        JitInstr::Nop,                // 7 (padding leader)
        JitInstr::OsrExit,            // 8 OSR-exit (was Return)
    ];
    let prog = f(1, 4, code);
    let id = m.compile_osr(&prog, 3, false, false).unwrap();
    // The window is `n_regs`-wide, indexed by register. Seed the loop live-in:
    // total=0 (reg1), i=1 (reg2), one=1 (reg3), n (reg0). lens parallel & unused.
    let run = |n: i64| -> NativeOutcome {
        let window = [n, 0, 1, 1]; // reg0=n, reg1=total, reg2=i, reg3=one
        let lens = [0i64; 4];
        m.call(id, &window, &lens)
    };
    for &(n, expected) in &[(10i64, 55i64), (0, 0), (100, 5050)] {
        match run(n) {
            NativeOutcome::Deopt {
                safepoint_id, live, ..
            } => {
                let site = m.deopt_map(id).unwrap().sites[safepoint_id.0 as usize - 1].clone();
                assert_eq!(site.resume_ip, 8, "OSR-exit resumes at the post-loop ip");
                let total = live
                    .iter()
                    .find(|r| r.reg == 1)
                    .map(|r| match r.value {
                        DeoptValue::Int(v) => v,
                        DeoptValue::Bool(_) => panic!("total is Int"),
                        DeoptValue::Float(_) => panic!("total is Int"),
                        DeoptValue::Handle(_) => panic!("total is Int"),
                    })
                    .expect("total is live-out");
                assert_eq!(total, expected, "live-out total for n={n}");
            }
            NativeOutcome::Completed(_) | NativeOutcome::CompletedHandle(_) => {
                panic!("OSR loop must deopt at exit, not complete")
            }
        }
    }
}

#[test]
fn osr_window_flat_list_direct_read_and_len() {
    // OSR loop summing a flat List<Int> that lives in a NON-param window slot
    // (reg index >= n_params). Models a loop-invariant typed list marshalled into
    // the OSR live-in window as a flat buffer: in-loop `List.get` lowers to
    // `ListGetIntDirect` and `List.len` to `ListLenDirect`, both basing off the
    // window register. This exercises the relaxed flat-base gate (admits an
    // OSR-window register, not only a top-level param).
    //
    // regs: 0=xs(FlatInt, NON-param window slot), 1=len, 2=i, 3=acc, 4=one, 5=elem
    // n_params=0. The flat list and every loop-carried value enter via the live-in
    // window (definite-assignment includes a register read-but-never-written in the
    // loop, exactly as `translate_osr_loop` produces for a pre-loop-built list whose
    // pre-header init instructions become `Bail` — no linear pred excludes it). The
    // pre-loop region is `Bail` (never run under OSR), so the header's only preds are
    // the loop backedge, keeping `xs`/`len`/`i`/`acc`/`one` definitely-assigned there.
    use JitValueType::{FlatInt, Int};
    let mut m = module();
    let code = vec![
        JitInstr::Bail, // 0 pre-loop (never run under OSR; no successor)
        JitInstr::Bail, // 1
        JitInstr::Bail, // 2
        JitInstr::Bail, // 3
        // 4: loop head (OSR header): if !(i < len) goto end(9)
        JitInstr::JumpIfIntCompare {
            lhs: 2,
            rhs: 1,
            op: JitCompare::Lt,
            expected: false,
            target: 9,
        },
        JitInstr::ListGetIntDirect {
            dst: 5,
            base: 0,
            index: 2,
        }, // 5 elem = xs[i]
        JitInstr::Add {
            dst: 3,
            lhs: 3,
            rhs: 5,
        }, // 6 acc += elem
        JitInstr::Add {
            dst: 2,
            lhs: 2,
            rhs: 4,
        }, // 7 i += 1
        JitInstr::Jump { target: 4 }, // 8 loop
        JitInstr::OsrExit,            // 9 exit (live-out: acc)
    ];
    let prog = ft(0, vec![FlatInt, Int, Int, Int, Int, Int], code);
    // OSR header at the loop head (ip 4). regs 0,1,2,3,4 are definitely assigned
    // there (read-only live-ins, never written in-loop).
    let id = m.compile_osr(&prog, 4, false, false).unwrap();

    let data: Vec<i64> = vec![10, 20, 30, 40];
    // The window is n_regs-wide; reg0's args slot holds the raw data pointer and
    // reg0's lens slot holds the element count (the flat-buffer ABI, by register).
    // Seed the loop live-in: len=4 (reg1), i=0 (reg2), acc=0 (reg3), one=1 (reg4).
    let mut window = [0i64; 6];
    window[0] = data.as_ptr() as i64;
    window[1] = data.len() as i64; // len (hoisted, live-in)
    window[2] = 0; // i
    window[3] = 0; // acc
    window[4] = 1; // one
    let mut lens = [0i64; 6];
    lens[0] = data.len() as i64;
    match m.call_with_host_ctx(id, &window, &lens, 0, &mut [FlatBufferArg::Int(&data)]) {
        NativeOutcome::Deopt {
            safepoint_id, live, ..
        } => {
            let site = m.deopt_map(id).unwrap().sites[safepoint_id.0 as usize - 1].clone();
            assert_eq!(site.resume_ip, 9, "exits at post-loop ip");
            let acc = live
                .iter()
                .find(|r| r.reg == 3)
                .map(|r| match r.value {
                    DeoptValue::Int(v) => v,
                    DeoptValue::Bool(_) => panic!("acc is Int"),
                    DeoptValue::Float(_) => panic!("acc is Int"),
                    DeoptValue::Handle(_) => panic!("acc is Int"),
                })
                .expect("acc is live-out");
            assert_eq!(acc, 100, "sum of [10,20,30,40] via direct reads");
        }
        other => panic!("OSR loop must deopt at exit, got {other:?}"),
    }
    // OOB safety: a window whose lens claims more elements than the buffer has
    // would read OOB — but every direct read is bounds-checked against lens, so a
    // shorter real loop bound (len) keeps every index in range. To prove the OOB
    // guard itself bails, force an index past the buffer by lying about len upward
    // is unsound for the test buffer; instead, an empty buffer (len 0) must make
    // the very first `i < len` false and exit immediately with acc=0.
    let mut empty_window = [0i64; 6];
    let empty: Vec<i64> = vec![];
    empty_window[0] = empty.as_ptr() as i64;
    empty_window[1] = 0; // len = 0
    empty_window[4] = 1; // one
    let mut empty_lens = [0i64; 6];
    empty_lens[0] = 0;
    match m.call_with_host_ctx(
        id,
        &empty_window,
        &empty_lens,
        0,
        &mut [FlatBufferArg::Int(&empty)],
    ) {
        NativeOutcome::Deopt {
            safepoint_id, live, ..
        } => {
            let site = m.deopt_map(id).unwrap().sites[safepoint_id.0 as usize - 1].clone();
            assert_eq!(site.resume_ip, 9);
            let acc = live.iter().find(|r| r.reg == 3).map(|r| match r.value {
                DeoptValue::Int(v) => v,
                DeoptValue::Bool(_) => panic!(),
                DeoptValue::Float(_) => panic!(),
                DeoptValue::Handle(_) => panic!(),
            });
            assert_eq!(acc, Some(0), "empty list sums to 0");
        }
        other => panic!("empty OSR loop must deopt at exit, got {other:?}"),
    }

    // ListLenDirect in-loop off a NON-param flat window register + OOB direct read.
    // regs: 0=xs(FlatInt), 1=i, 2=acc, 3=one, 4=len, 5=elem. The loop reads len
    // directly each iteration (ListLenDirect base=0) and indexes xs[i]; an `i`
    // pushed past `len` (here we drive the loop bound from a SEPARATE register `b`
    // larger than the buffer) makes the direct read OOB ⇒ a bounds-check bail/deopt
    // (NOT UB), matching the host helper's OOB bail.
    // regs: 0=xs, 1=i, 2=acc, 3=one, 4=len(unused-here), 5=elem, 6=bound
    let code2 = vec![
        JitInstr::Bail, // 0 pre-loop
        JitInstr::Bail, // 1
        JitInstr::Bail, // 2
        JitInstr::Bail, // 3
        // 4: header: if !(i < bound) goto end(10)
        JitInstr::JumpIfIntCompare {
            lhs: 1,
            rhs: 6,
            op: JitCompare::Lt,
            expected: false,
            target: 10,
        },
        JitInstr::ListLenDirect { dst: 4, base: 0 }, // 5 len = len(xs)  (direct)
        JitInstr::ListGetIntDirect {
            dst: 5,
            base: 0,
            index: 1,
        }, // 6 elem = xs[i] (OOB once i>=len)
        JitInstr::Add {
            dst: 2,
            lhs: 2,
            rhs: 5,
        }, // 7 acc += elem
        JitInstr::Add {
            dst: 1,
            lhs: 1,
            rhs: 3,
        }, // 8 i += 1
        JitInstr::Jump { target: 4 },                // 9 loop
        JitInstr::OsrExit,                           // 10 exit
    ];
    let prog2 = ft(
        0,
        vec![JitValueType::FlatInt, Int, Int, Int, Int, Int, Int],
        code2,
    );
    let id2 = m.compile_osr(&prog2, 4, false, false).unwrap();
    // Drive bound=8 but the buffer only has 4 elements ⇒ at i==4 the direct read is
    // OOB and must deopt (a bounds bail), never reading past the buffer.
    let mut w2 = [0i64; 7];
    w2[0] = data.as_ptr() as i64; // xs
    w2[1] = 0; // i
    w2[2] = 0; // acc
    w2[3] = 1; // one
    w2[6] = 8; // bound (> len 4)
    let mut l2 = [0i64; 7];
    l2[0] = data.len() as i64; // lens[xs] = 4 — the bounds-check source
    match m.call_with_host_ctx(id2, &w2, &l2, 0, &mut [FlatBufferArg::Int(&data)]) {
        NativeOutcome::Deopt { safepoint_id, .. } => {
            let site = m.deopt_map(id2).unwrap().sites[safepoint_id.0 as usize - 1].clone();
            // The OOB read bails at the ListGetIntDirect ip (6), NOT the exit (10):
            // a precise mid-loop deopt, so the interpreter re-runs and raises the
            // real out-of-bounds behavior itself.
            assert_eq!(
                site.resume_ip, 6,
                "OOB direct read bails at its own ip (not UB)"
            );
        }
        other => panic!("OOB direct read must deopt, got {other:?}"),
    }
}

#[test]
fn osr_flat_base_gate_rejects_nonwindow_non_osr() {
    // The relaxed flat-base gate: under OSR a flat base may be any register in the
    // n_regs-wide window (index >= n_params); under a NORMAL compile it must still
    // be a packed param (index < n_params). A non-OSR program with a flat base at a
    // non-param register is rejected by validation.
    use JitValueType::{FlatInt, Int};
    // n_params=1 (reg0 the only param), reg2 is a FlatInt non-param ⇒ illegal base
    // for a normal compile.
    let prog = ft(
        1,
        vec![Int, Int, FlatInt, Int],
        vec![
            JitInstr::ListGetIntDirect {
                dst: 1,
                base: 2,
                index: 0,
            },
            JitInstr::Return { src: 1 },
        ],
    );
    assert!(
        crate::validate(&prog, false).is_err(),
        "non-param flat base must be rejected by a normal compile"
    );
    // Under OSR the same dataflow shape validates once it uses the OSR exit
    // contract (the window is n_regs-wide).
    let osr_prog = ft(
        1,
        vec![Int, Int, FlatInt, Int],
        vec![
            JitInstr::ListGetIntDirect {
                dst: 1,
                base: 2,
                index: 0,
            },
            JitInstr::OsrExit,
        ],
    );
    assert!(
        crate::validate(&osr_prog, true).is_ok(),
        "an OSR-window flat base (index >= n_params) must validate"
    );
}

#[test]
fn osr_rejects_non_leader_header() {
    // An OSR header ip that is not a leader / jump-target block is rejected
    // cleanly (no panic, no miscompile).
    let mut m = module();
    let prog = f(
        1,
        2,
        vec![
            JitInstr::Add {
                dst: 1,
                lhs: 0,
                rhs: 0,
            },
            JitInstr::Return { src: 1 },
        ],
    );
    // ip 1 (the Return) is a leader only if a jump targets it; here none does,
    // and it is not ip 0, so it has no block.
    assert!(m.compile_osr(&prog, 1, false, false).is_err());
}

#[test]
fn div_by_zero_bails() {
    let mut m = module();
    let id = m
        .compile(&f(
            2,
            3,
            vec![
                JitInstr::Div {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    assert_eq!(m.callt(id, &[20, 5]), Some(4));
    assert_eq!(m.callt(id, &[20, 0]), None);
    assert_eq!(m.callt(id, &[i64::MIN, -1]), None);
}

// --- deopt live-register capture: live-register value capture at deopt --------------------------

/// Find the captured value of register `reg` in a deopt outcome's `live` set.
fn live_value(outcome: &NativeOutcome, reg: u32) -> Option<DeoptValue> {
    match outcome {
        NativeOutcome::Deopt { live, .. } => live.iter().find(|r| r.reg == reg).map(|r| r.value),
        NativeOutcome::Completed(_) | NativeOutcome::CompletedHandle(_) => None,
    }
}

#[test]
fn deopt_capture_records_live_register_values() {
    use JitValueType::{FlatInt, Int};
    let mut m = module();
    // fn(xs: FlatInt, a: Int, b: Int) -> Int { t = a + b; return xs[t] }
    // regs 0=xs, 1=a, 2=b, 3=t. The `ListGetIntDirect` OOB guard (ip 1) resumes
    // with xs(0)/a(1)/b(2)/t(3) all definitely assigned on entry.
    let id = m
        .compile(&ft(
            3,
            vec![FlatInt, Int, Int, Int],
            vec![
                JitInstr::Add {
                    dst: 3,
                    lhs: 1,
                    rhs: 2,
                },
                JitInstr::ListGetIntDirect {
                    dst: 3,
                    base: 0,
                    index: 3,
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();
    let xs: Vec<i64> = vec![10, 20, 30, 40, 50];
    let xs_ptr = xs.as_ptr() as i64;
    let xlen = xs.len() as i64;

    // In range: t = 1 + 2 = 3 → xs[3] = 40.
    assert_eq!(
        m.call_with_host_ctx(
            id,
            &[xs_ptr, 1, 2],
            &[xlen, 0, 0],
            0,
            &mut [FlatBufferArg::Int(&xs)]
        ),
        NativeOutcome::Completed(40)
    );

    // Out of range: t = 3 + 4 = 7 >= len 5 → the direct-read OOB guard bails.
    let out = m.call_with_host_ctx(
        id,
        &[xs_ptr, 3, 4],
        &[xlen, 0, 0],
        0,
        &mut [FlatBufferArg::Int(&xs)],
    );
    assert!(matches!(out, NativeOutcome::Deopt { .. }));
    // t (reg 3) was computed before the guard fired and is captured.
    assert_eq!(live_value(&out, 3), Some(DeoptValue::Int(7)));
    // The params a (reg 1) and b (reg 2) are captured with their passed values.
    assert_eq!(live_value(&out, 1), Some(DeoptValue::Int(3)));
    assert_eq!(live_value(&out, 2), Some(DeoptValue::Int(4)));
}

#[test]
fn deopt_capture_records_float_register_value() {
    use JitValueType::{FlatInt, Float, Int};
    let mut m = module();
    // fn(xs: FlatInt, i: Int, f: Float) -> Int { g = f + f; return xs[i] }
    // regs 0=xs, 1=i, 2=f, 3=g. The float `g` is definitely assigned before the
    // `ListGetIntDirect` OOB guard (ip 2), so it is captured as an exact f64.
    let id = m
        .compile(&ft(
            3,
            vec![FlatInt, Int, Float, Float],
            vec![
                JitInstr::Add {
                    dst: 3,
                    lhs: 2,
                    rhs: 2,
                },
                JitInstr::ListGetIntDirect {
                    dst: 1,
                    base: 0,
                    index: 1,
                },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();
    let xs: Vec<i64> = vec![7];
    let xs_ptr = xs.as_ptr() as i64;
    let xlen = xs.len() as i64;
    let f = 1.5_f64;

    // Out of range index 9 → bail; the float g = f + f = 3.0 round-trips exactly.
    let out = m.call_with_host_ctx(
        id,
        &[xs_ptr, 9, f.to_bits() as i64],
        &[xlen, 0, 0],
        0,
        &mut [FlatBufferArg::Int(&xs)],
    );
    assert!(matches!(out, NativeOutcome::Deopt { .. }));
    assert_eq!(live_value(&out, 3), Some(DeoptValue::Float(f + f)));
    // The float param f itself is captured exactly too.
    assert_eq!(live_value(&out, 2), Some(DeoptValue::Float(f)));
}

// --- forced-deopt stress: deopt-at-every-safepoint stress test (master correctness) ------

/// Force a bail at EVERY safepoint of a few representative functions and verify
/// the captured safepoint id + live register values are correct at each — even at
/// safepoints that never fire under the (deliberately in-range, non-overflowing)
/// inputs. This exercises the deopt capture/map machinery exhaustively: for every
/// site `k`, `compile_forcing_bail(f, k)` makes only site `k` bail, and we assert
/// the outcome is `Deopt { SafepointId(k) }` whose `live` set is exactly the one
/// `deopt_map().sites[k-1]` advertises, each register carrying the value the
/// function computes for it. Late sites must capture earlier intermediates.
#[test]
fn force_bail_at_every_safepoint_captures_correct_state() {
    use JitValueType::{FlatInt, Float, Int};

    // A representative case: a `JitFunction`, the in-range inputs (args/lens) that
    // make NO natural bail fire, and a closure giving the value the function
    // computes for each register at any safepoint, so we can check every capture.
    struct Case {
        name: &'static str,
        func: JitFunction,
        args: Vec<i64>,
        lens: Vec<i64>,
        // reg -> captured DeoptValue, for any reg appearing in a site's live set.
        expect: Box<dyn Fn(u32) -> DeoptValue>,
    }

    let ints: Vec<i64> = vec![10, 20, 30, 40, 50];
    let ints_ptr = ints.as_ptr() as i64;
    let ilen = ints.len() as i64;

    // Case A: fn(a: FlatInt, x: Int, i: Int) { t = x + x; return a[i] }
    // Sites: 1 = Add overflow guard (ip 0), 2 = ListGetIntDirect OOB (ip 1).
    // Site 2 is LATE and captures the earlier-computed t = x + x.
    let a_ptr = ints_ptr;
    let case_a = Case {
        name: "add-then-direct-get",
        func: ft(
            3,
            vec![FlatInt, Int, Int, Int, Int],
            vec![
                JitInstr::Add {
                    dst: 3,
                    lhs: 1,
                    rhs: 1,
                },
                JitInstr::ListGetIntDirect {
                    dst: 4,
                    base: 0,
                    index: 2,
                },
                JitInstr::Return { src: 4 },
            ],
        ),
        // a = ptr, x = 7, i = 2 (in range). t = x + x = 14.
        args: vec![a_ptr, 7, 2],
        lens: vec![ilen, 0, 0],
        expect: Box::new(|reg| match reg {
            0 => DeoptValue::Int(0), // a: ptr value is asserted separately
            1 => DeoptValue::Int(7),
            2 => DeoptValue::Int(2),
            3 => DeoptValue::Int(14),
            _ => DeoptValue::Int(0),
        }),
    };

    // Case B: fn(xs: FlatInt, i: Int, f: Float) { g = f + f; return xs[i] }
    // The float Add has no guard, so the only site is the ListGetIntDirect OOB
    // (ip 1). Its live set includes the float register g — checked as exact f64.
    let xs_ptr = ints_ptr;
    let fv = 1.25_f64;
    let case_b = Case {
        name: "float-reg-direct-get",
        func: ft(
            3,
            vec![FlatInt, Int, Float, Float],
            vec![
                JitInstr::Add {
                    dst: 3,
                    lhs: 2,
                    rhs: 2,
                },
                JitInstr::ListGetIntDirect {
                    dst: 1,
                    base: 0,
                    index: 1,
                },
                JitInstr::Return { src: 1 },
            ],
        ),
        // xs = ptr, i = 1 (in range), f = 1.25. g = f + f = 2.5.
        args: vec![xs_ptr, 1, fv.to_bits() as i64],
        lens: vec![ilen, 0, 0],
        expect: Box::new(move |reg| match reg {
            1 => DeoptValue::Int(1),
            2 => DeoptValue::Float(fv),
            3 => DeoptValue::Float(fv + fv),
            _ => DeoptValue::Int(0),
        }),
    };

    // Case C: fn(a: FlatInt, x: Int, y: Int) { p = x + y; q = p * x; return a[y] }
    // Three sites: 1 = Add (ip 0), 2 = Mul (ip 1), 3 = ListGetIntDirect OOB (ip 2).
    // The LATE site 3 captures both earlier intermediates p and q.
    let case_c = Case {
        name: "add-mul-then-direct-get",
        func: ft(
            3,
            vec![FlatInt, Int, Int, Int, Int, Int],
            vec![
                JitInstr::Add {
                    dst: 3,
                    lhs: 1,
                    rhs: 2,
                },
                JitInstr::Mul {
                    dst: 4,
                    lhs: 3,
                    rhs: 1,
                },
                JitInstr::ListGetIntDirect {
                    dst: 5,
                    base: 0,
                    index: 2,
                },
                JitInstr::Return { src: 5 },
            ],
        ),
        // a = ptr, x = 3, y = 4 (in range). p = 3 + 4 = 7, q = p * x = 21.
        args: vec![ints_ptr, 3, 4],
        lens: vec![ilen, 0, 0],
        expect: Box::new(|reg| match reg {
            1 => DeoptValue::Int(3),
            2 => DeoptValue::Int(4),
            3 => DeoptValue::Int(7),  // p
            4 => DeoptValue::Int(21), // q
            _ => DeoptValue::Int(0),
        }),
    };

    let cases = [case_a, case_b, case_c];
    let mut combinations = 0usize;
    let mut late_intermediate_checks = 0usize;

    for case in &cases {
        let mut m = module();
        // Site count from the natural (un-forced) compilation.
        let base_id = m.compile(&case.func).unwrap();
        let n = m.deopt_map(base_id).expect("map for valid id").sites.len();
        assert!(n >= 1, "{}: expected at least one safepoint", case.name);

        for k in 1..=n as u32 {
            // Force ONLY site k to bail; inputs are chosen so no natural bail fires.
            let id = m.compile_forcing_bail(&case.func, k).unwrap();
            let site = m.deopt_map(id).expect("map").sites[(k - 1) as usize].clone();
            let out = m.call_with_host_ctx(
                id,
                &case.args,
                &case.lens,
                0,
                &mut [FlatBufferArg::Int(&ints)],
            );

            // The forced site must bail with exactly its id.
            let live = match &out {
                NativeOutcome::Deopt {
                    safepoint_id, live, ..
                } => {
                    assert_eq!(
                        *safepoint_id,
                        SafepointId(k),
                        "{}: forced site {} reported wrong safepoint id",
                        case.name,
                        k
                    );
                    live
                }
                NativeOutcome::Completed(_) | NativeOutcome::CompletedHandle(_) => {
                    panic!("{}: forced site {} did not bail", case.name, k)
                }
            };

            // Heap-aware deopt (heap-aware deopt): the captured live set is exactly the SCALAR
            // (`Int`/`Float`) subset of the map's live set — `Handle`/`FlatInt`/
            // `FlatFloat` regs are reconstructed from the interpreter frame, not the
            // payload, so they are intentionally absent from the capture.
            let mut captured: Vec<u32> = live.iter().map(|r| r.reg).collect();
            captured.sort_unstable();
            let mut expected_regs: Vec<u32> = site
                .live
                .iter()
                .filter(|(_, ty)| matches!(ty, JitValueType::Int | JitValueType::Float))
                .map(|(r, _)| *r)
                .collect();
            expected_regs.sort_unstable();
            assert_eq!(
                captured, expected_regs,
                "{}: site {} scalar live-reg set mismatch (map vs capture)",
                case.name, k
            );

            // ...each captured SCALAR value must match what the function computes;
            // a non-scalar (Handle/FlatInt/FlatFloat) reg is NOT reconstructed and
            // must be absent from the capture.
            for &(reg, ty) in &site.live {
                match ty {
                    JitValueType::Int | JitValueType::Bool | JitValueType::Float => {
                        let got = live_value(&out, reg).expect("captured scalar reg present");
                        assert_eq!(
                            got,
                            (case.expect)(reg),
                            "{}: site {} reg {} value mismatch",
                            case.name,
                            k,
                            reg
                        );
                    }
                    JitValueType::Handle
                    | JitValueType::FlatInt
                    | JitValueType::FlatIntMut
                    | JitValueType::FlatFloat
                    | JitValueType::FlatFloatMut => {
                        assert!(
                            live_value(&out, reg).is_none(),
                            "{}: site {} reg {} (non-scalar) must not be reconstructed",
                            case.name,
                            k,
                            reg
                        );
                    }
                }
            }

            combinations += 1;
        }

        // Explicit late-site check: the LAST site of a multi-site function must
        // capture an earlier-computed intermediate with its correct value.
        if n >= 2 {
            let id = m.compile_forcing_bail(&case.func, n as u32).unwrap();
            let out = m.call_with_host_ctx(
                id,
                &case.args,
                &case.lens,
                0,
                &mut [FlatBufferArg::Int(&ints)],
            );
            // reg 3 is the first arithmetic result in cases A and C; it is computed
            // at an earlier site yet must be captured at the final site.
            assert_eq!(
                live_value(&out, 3),
                Some((case.expect)(3)),
                "{}: late site {} failed to capture earlier intermediate reg 3",
                case.name,
                n
            );
            late_intermediate_checks += 1;
        }
    }

    // Sanity: we actually exercised every site of every case plus late checks.
    assert_eq!(combinations, 2 + 1 + 3, "unexpected (case, site) coverage");
    assert!(late_intermediate_checks >= 2, "expected late-site checks");
}

#[test]
fn compile_forcing_all_bails_deopts_at_first_executed_safepoint() {
    use JitValueType::{FlatInt, Int};

    let values = [10, 20, 30];
    let ptr = values.as_ptr() as i64;
    let func = ft(
        3,
        vec![FlatInt, Int, Int, Int, Int],
        vec![
            JitInstr::Add {
                dst: 3,
                lhs: 1,
                rhs: 1,
            },
            JitInstr::ListGetIntDirect {
                dst: 4,
                base: 0,
                index: 2,
            },
            JitInstr::Return { src: 4 },
        ],
    );

    let mut m = module();
    let id = m.compile_forcing_all_bails(&func).unwrap();
    assert_eq!(
        m.deopt_map(id).expect("map").sites.len(),
        2,
        "test function should have both add-overflow and direct-list guards",
    );
    let out = m.call_with_host_ctx(
        id,
        &[ptr, 7, 1],
        &[values.len() as i64, 0, 0],
        0,
        &mut [FlatBufferArg::Int(&values)],
    );
    match out {
        NativeOutcome::Deopt {
            safepoint_id, live, ..
        } => {
            assert_eq!(safepoint_id, SafepointId(1));
            assert_eq!(
                live.iter().find(|reg| reg.reg == 1).map(|reg| reg.value),
                Some(DeoptValue::Int(7))
            );
        }
        other => panic!("expected forced all-sites deopt, got {other:?}"),
    }
}
use super::*;
