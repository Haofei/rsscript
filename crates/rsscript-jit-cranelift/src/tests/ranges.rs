// --- interval range analysis: conservative range proof for eliding overflow checks -----------

/// LoadInt c ⇒ [c, c]; an Add of two known constants whose sum fits i64 is
/// proven non-overflowing (and the proof line is computed in i128).
#[test]
fn interval_load_and_add_constants() {
    let prog = f(
        0,
        3,
        vec![
            JitInstr::LoadInt { dst: 0, value: 5 },
            JitInstr::LoadInt { dst: 1, value: 3 },
            JitInstr::Add {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            JitInstr::Return { src: 2 },
        ],
    );
    let iv = interval_analysis(&prog);
    // On entry to the Add (ip 2), reg0=[5,5], reg1=[3,3].
    assert_eq!(iv[2][0], Interval { lo: 5, hi: 5 });
    assert_eq!(iv[2][1], Interval { lo: 3, hi: 3 });
    // The Add is proven non-overflowing ([8,8] ⊂ i64).
    assert!(arith_cannot_overflow(&iv[2], &prog.code[2]));
    // The result [8,8] flows to the Return's in-set (ip 3).
    assert_eq!(iv[3][2], Interval { lo: 8, hi: 8 });
}

/// A proven-unchecked add of two large constants whose sum is STILL within i64
/// produces the exact (non-wrapping) sum — the unchecked op is byte-identical to
/// the checked one here because the proof guarantees no overflow.
#[test]
fn proven_large_constant_add_is_correct() {
    let mut m = module();
    // c = (i64::MAX - 10) + 10 = i64::MAX, proven safe ⇒ unchecked, exact.
    let id = m
        .compile(&f(
            0,
            3,
            vec![
                JitInstr::LoadInt {
                    dst: 0,
                    value: i64::MAX - 10,
                },
                JitInstr::LoadInt { dst: 1, value: 10 },
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    assert_eq!(m.callt(id, &[]), Some(i64::MAX));
}

/// Boundary: operand ranges summing to EXACTLY i64::MAX are proven safe; summing
/// to i64::MAX + 1 are NOT (the analysis draws the line at the i64 boundary).
#[test]
fn boundary_exact_max_proven_overflow_unproven() {
    // Proven: (i64::MAX - 1) + 1 = i64::MAX, fits ⇒ unchecked.
    let safe = f(
        0,
        3,
        vec![
            JitInstr::LoadInt {
                dst: 0,
                value: i64::MAX - 1,
            },
            JitInstr::LoadInt { dst: 1, value: 1 },
            JitInstr::Add {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            JitInstr::Return { src: 2 },
        ],
    );
    let iv = interval_analysis(&safe);
    assert_eq!(
        iv[2][0],
        Interval {
            lo: i64::MAX as i128 - 1,
            hi: i64::MAX as i128 - 1
        }
    );
    assert!(arith_cannot_overflow(&iv[2], &safe.code[2]));

    // Just over: i64::MAX + 1 overflows ⇒ NOT proven, stays checked.
    let over = f(
        0,
        3,
        vec![
            JitInstr::LoadInt {
                dst: 0,
                value: i64::MAX,
            },
            JitInstr::LoadInt { dst: 1, value: 1 },
            JitInstr::Add {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            JitInstr::Return { src: 2 },
        ],
    );
    let iv2 = interval_analysis(&over);
    assert!(!arith_cannot_overflow(&iv2[2], &over.code[2]));
}

/// The proven-boundary add runs unchecked and yields exactly i64::MAX (no bail);
/// the over-boundary constant add — which the proof leaves CHECKED — bails on its
/// actual overflow. Same analysis, opposite emission, both correct.
#[test]
fn boundary_proven_runs_overflow_constant_bails() {
    let mut m = module();
    let safe = m
        .compile(&f(
            0,
            3,
            vec![
                JitInstr::LoadInt {
                    dst: 0,
                    value: i64::MAX - 1,
                },
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    assert_eq!(m.callt(safe, &[]), Some(i64::MAX));

    let over = m
        .compile(&f(
            0,
            3,
            vec![
                JitInstr::LoadInt {
                    dst: 0,
                    value: i64::MAX,
                },
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    // Constant operands are tracked, [i64::MAX]+[1] doesn't fit ⇒ stays checked ⇒
    // the sadd_overflow guard fires on the real overflow ⇒ Deopt (None).
    assert_eq!(m.callt(over, &[]), None);
}

/// Params are untracked (TOP). `a + b` over params stays CHECKED and bails on a
/// real overflow (i64::MAX + 1) exactly as before — proving checks are NOT
/// over-eagerly stripped.
#[test]
fn unknown_params_stay_checked_and_bail() {
    let mut m = module();
    // fn(a, b) -> Int { return a + b }, params untracked ⇒ TOP ⇒ checked.
    let prog = f(
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
    );
    let iv = interval_analysis(&prog);
    // Both operands TOP ⇒ result range is TOP ⇒ not proven.
    assert!(!arith_cannot_overflow(&iv[0], &prog.code[0]));
    let id = m.compile(&prog).unwrap();
    // In-range add returns the exact sum.
    assert_eq!(m.callt(id, &[2, 3]), Some(5));
    // i64::MAX + 1 overflows ⇒ the retained check bails.
    assert_eq!(m.callt(id, &[i64::MAX, 1]), None);
}

#[test]
fn unreachable_predecessors_cannot_narrow_entry_parameters() {
    let cases = [
        (
            "add",
            JitInstr::Add {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            [i64::MAX, 1],
        ),
        (
            "sub",
            JitInstr::Sub {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            [i64::MIN, 1],
        ),
        (
            "mul",
            JitInstr::Mul {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            [i64::MAX, 2],
        ),
    ];

    for (name, arithmetic, args) in cases {
        let program = f(
            2,
            3,
            vec![
                arithmetic,
                JitInstr::Return { src: 2 },
                JitInstr::LoadInt { dst: 0, value: 0 },
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Jump { target: 0 },
            ],
        );
        let intervals = interval_analysis(&program);
        assert_eq!(intervals[0][0], Interval::TOP, "{name}");
        assert_eq!(intervals[0][1], Interval::TOP, "{name}");
        assert!(
            !arith_cannot_overflow(&intervals[0], &program.code[0]),
            "{name}"
        );

        let mut module = module();
        let id = module.compile(&program).expect(name);
        assert_eq!(module.callt(id, &args), None, "{name}");
    }
}

#[test]
fn reachable_backedge_cannot_narrow_virtual_entry_parameters() {
    use JitValueType::{Bool, Int};

    let program = ft(
        3,
        vec![Int, Int, Bool, Int],
        vec![
            JitInstr::Add {
                dst: 3,
                lhs: 0,
                rhs: 1,
            },
            JitInstr::JumpIfBool {
                cond: 2,
                expected: true,
                target: 5,
            },
            JitInstr::LoadInt { dst: 0, value: 0 },
            JitInstr::LoadInt { dst: 1, value: 1 },
            JitInstr::Jump { target: 0 },
            JitInstr::Return { src: 3 },
        ],
    );
    let intervals = interval_analysis(&program);
    assert_eq!(intervals[0][0], Interval::TOP);
    assert_eq!(intervals[0][1], Interval::TOP);
    assert!(!arith_cannot_overflow(&intervals[0], &program.code[0]));

    let mut module = module();
    let id = module.compile(&program).expect("program should compile");
    assert_eq!(module.callt(id, &[i64::MAX, 1, 1]), None);
}

/// A register fed by `ListLen` is `[0, i64::MAX]` — non-negative but with NO
/// tighter upper bound. So `len + len` can reach 2*i64::MAX, does NOT fit i64,
/// and stays CHECKED (we did not assume a smaller length bound).
#[test]
fn list_len_is_nonneg_unbounded_above() {
    use JitValueType::{Handle, Int};
    let prog = ft(
        1,
        vec![Handle, Int, Int],
        vec![
            JitInstr::HostCall {
                helper: HostHelper::ListLen,
                dst: 1,
                args: vec![HostArg::Reg(0)],
            },
            // len + len
            JitInstr::Add {
                dst: 2,
                lhs: 1,
                rhs: 1,
            },
            JitInstr::Return { src: 2 },
        ],
    );
    let iv = interval_analysis(&prog);
    // ListLen result is exactly [0, i64::MAX] on entry to the Add (ip 1).
    assert_eq!(
        iv[1][1],
        Interval {
            lo: 0,
            hi: i64::MAX as i128
        }
    );
    // [0,MAX]+[0,MAX] = [0, 2*MAX] does NOT fit i64 ⇒ stays checked.
    assert!(!arith_cannot_overflow(&iv[1], &prog.code[1]));
}

/// ListLenDirect is treated identically to ListLen: [0, i64::MAX].
#[test]
fn list_len_direct_is_nonneg() {
    use JitValueType::{FlatInt, Int};
    let prog = ft(
        1,
        vec![FlatInt, Int],
        vec![
            JitInstr::ListLenDirect { dst: 1, base: 0 },
            JitInstr::Return { src: 1 },
        ],
    );
    let iv = interval_analysis(&prog);
    assert_eq!(
        iv[1][1],
        Interval {
            lo: 0,
            hi: i64::MAX as i128
        }
    );
}

/// Sub and Mul interval transfer functions, plus their proven/unproven lines.
#[test]
fn interval_sub_and_mul_transfer() {
    // (10 - 3) = 7, proven; Move copies a range.
    let prog = f(
        0,
        4,
        vec![
            JitInstr::LoadInt { dst: 0, value: 10 },
            JitInstr::LoadInt { dst: 1, value: 3 },
            JitInstr::Sub {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            JitInstr::Mul {
                dst: 3,
                lhs: 2,
                rhs: 1,
            },
            JitInstr::Return { src: 3 },
        ],
    );
    let iv = interval_analysis(&prog);
    assert!(arith_cannot_overflow(&iv[2], &prog.code[2])); // [10,10]-[3,3]=[7,7]
    assert_eq!(iv[3][2], Interval { lo: 7, hi: 7 });
    assert!(arith_cannot_overflow(&iv[3], &prog.code[3])); // [7,7]*[3,3]=[21,21]

    // A Mul of two large constants whose product overflows is NOT proven.
    let big = f(
        0,
        3,
        vec![
            JitInstr::LoadInt {
                dst: 0,
                value: i64::MAX,
            },
            JitInstr::LoadInt { dst: 1, value: 2 },
            JitInstr::Mul {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            JitInstr::Return { src: 2 },
        ],
    );
    let iv2 = interval_analysis(&big);
    assert!(!arith_cannot_overflow(&iv2[2], &big.code[2]));
}

/// A register incremented across a loop back-edge widens to TOP (we infer no loop
/// bound), so an unbounded accumulator's add stays CHECKED — and the fixpoint
/// terminates.
#[test]
fn loop_accumulator_widens_to_top() {
    // i = 0; loop { i = i + 1; } (no exit) — i grows unbounded.
    // regs: 0=i, 1=one
    let prog = f(
        0,
        2,
        vec![
            JitInstr::LoadInt { dst: 0, value: 0 }, // 0
            JitInstr::LoadInt { dst: 1, value: 1 }, // 1
            JitInstr::Add {
                dst: 0,
                lhs: 0,
                rhs: 1,
            }, // 2: i = i + 1
            JitInstr::Jump { target: 2 },           // 3: back-edge to the Add
        ],
    );
    let iv = interval_analysis(&prog);
    // After widening, `i` on entry to the Add is TOP ⇒ the increment stays checked.
    assert_eq!(iv[2][0], Interval::TOP);
    assert!(!arith_cannot_overflow(&iv[2], &prog.code[2]));
}

// --- branch-conditioned range refinement: branch-conditioned range refinement for loop counters ----------

/// Build the counted loop
///   fn f(limit){ i=0; total=0; while i<limit { total += step; i += incr }; total }
/// in JIT IR. regs: 0=limit(param), 1=i, 2=total, 3=incr(const), 4=step(const 1).
/// The guard is `JumpIfIntCompare i < limit, expected=false, target=exit`, so the
/// FALL-THROUGH edge into the body asserts `i < limit` (the refinement site).
fn counted_loop(incr: i64, op: JitCompare) -> JitFunction {
    f(
        1,
        5,
        vec![
            JitInstr::LoadInt { dst: 1, value: 0 }, // 0: i = 0
            JitInstr::LoadInt { dst: 2, value: 0 }, // 1: total = 0
            JitInstr::LoadInt {
                dst: 3,
                value: incr,
            }, // 2: incr
            JitInstr::LoadInt { dst: 4, value: 1 }, // 3: step = 1
            // 4: header guard — if !(i <op> limit) goto exit(8)
            JitInstr::JumpIfIntCompare {
                lhs: 1,
                rhs: 0,
                op,
                expected: false,
                target: 8,
            },
            JitInstr::Add {
                dst: 2,
                lhs: 2,
                rhs: 4,
            }, // 5: total = total + 1 (unbounded)
            JitInstr::Add {
                dst: 1,
                lhs: 1,
                rhs: 3,
            }, // 6: i = i + incr (loop counter)
            JitInstr::Jump { target: 4 }, // 7: back-edge
            JitInstr::Return { src: 2 },  // 8: exit
        ],
    )
}

/// (i) Under the guard `i < limit`, the counter increment `i = i + 1` is proven
/// UNCHECKED: on the loop-body edge `i <= limit - 1 <= i64::MAX - 1`, so
/// `i + 1 <= i64::MAX` provably fits. The unbounded accumulator `total = total + 1`
/// stays CHECKED (total widens to TOP — no bounding guard).
#[test]
fn loop_counter_lt_increment_proven_accumulator_checked() {
    let prog = counted_loop(1, JitCompare::Lt);
    let iv = interval_analysis(&prog);
    // Body in-set (ip 5/6): the refined `i` is bounded above by limit.hi - 1.
    // limit is TOP ([MIN, MAX]) ⇒ i.hi = MAX - 1.
    assert_eq!(iv[6][1].hi, i64::MAX as i128 - 1);
    // i = i + 1 (ip 6): [.., MAX-1] + [1,1] = [.., MAX] ⇒ fits ⇒ UNCHECKED.
    assert!(arith_cannot_overflow(&iv[6], &prog.code[6]));
    // total = total + 1 (ip 5): total is TOP ⇒ stays CHECKED.
    assert!(!arith_cannot_overflow(&iv[5], &prog.code[5]));
}

/// The same loop, compiled and run: the result equals the loop trip count for a
/// large limit, confirming the UNCHECKED counter increment is correct at scale
/// (no spurious bail, no wrong wrap). total stays small so its checked add is fine.
#[test]
fn loop_counter_lt_runs_correct_at_scale() {
    let mut m = module();
    let id = m.compile(&counted_loop(1, JitCompare::Lt)).unwrap();
    assert_eq!(m.callt(id, &[0]), Some(0));
    assert_eq!(m.callt(id, &[1]), Some(1));
    assert_eq!(m.callt(id, &[1_000_000]), Some(1_000_000));
}

/// (ii-a) `i = i + 2` must stay CHECKED: under `i < limit` we only know
/// `i <= i64::MAX - 1`, so `i + 2` can reach `i64::MAX + 1` ⇒ does NOT fit.
#[test]
fn loop_counter_plus_two_stays_checked() {
    let prog = counted_loop(2, JitCompare::Lt);
    let iv = interval_analysis(&prog);
    // i still refined to [.., MAX-1], but [.., MAX-1] + [2,2] = [.., MAX+1] ⇒
    // does NOT fit i64 ⇒ CHECKED.
    assert_eq!(iv[6][1].hi, i64::MAX as i128 - 1);
    assert!(!arith_cannot_overflow(&iv[6], &prog.code[6]));
}

/// (ii-b) `while i <= limit` must keep `i = i + 1` CHECKED: the `Le` taken-edge
/// only proves `i <= limit <= i64::MAX`, so `i` may BE `i64::MAX` and `i + 1`
/// overflows. This locks the Lt-vs-Le off-by-one.
#[test]
fn loop_counter_le_increment_stays_checked() {
    let prog = counted_loop(1, JitCompare::Le);
    let iv = interval_analysis(&prog);
    // Under `i <= limit` (limit TOP), i.hi = min(MAX, limit.hi) = MAX, NOT MAX-1.
    assert_eq!(iv[6][1].hi, i64::MAX as i128);
    // [.., MAX] + [1,1] = [.., MAX+1] ⇒ does NOT fit ⇒ CHECKED.
    assert!(!arith_cannot_overflow(&iv[6], &prog.code[6]));
}

/// (iii) A bare `a + b` with unconstrained (TOP) operands and no governing guard
/// stays CHECKED and bails on a real overflow — refinement never strips a check
/// when there is no comparison fact to refine by. (Mirrors the param test, kept
/// here so the branch-conditioned range refinement slice asserts the negative directly.)
#[test]
fn unguarded_add_stays_checked_and_bails() {
    let mut m = module();
    let prog = f(
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
    );
    let iv = interval_analysis(&prog);
    assert!(!arith_cannot_overflow(&iv[0], &prog.code[0]));
    let id = m.compile(&prog).unwrap();
    assert_eq!(m.callt(id, &[i64::MAX, 1]), None); // checked overflow ⇒ bail
}

/// The refinement is edge-SENSITIVE: the SAME register is bounded on the taken
/// edge but TOP at the post-join loop header. A direct two-block check that the
/// guard's taken (`<`) edge tightens lhs while the header stays TOP, and that an
/// unreachable refinement (`x < x`) is handled soundly (no malformed interval).
#[test]
fn refinement_edge_sensitive_and_unreachable_safe() {
    // if i < limit { return i + 1 } else { return i }   regs 0=limit,1=i,2=t
    let prog = f(
        2,
        3,
        vec![
            // 0: if !(i < limit) goto 3 (else-branch)
            JitInstr::JumpIfIntCompare {
                lhs: 1,
                rhs: 0,
                op: JitCompare::Lt,
                expected: false,
                target: 3,
            },
            JitInstr::Add {
                dst: 2,
                lhs: 1,
                rhs: 1,
            }, // 1: t = i + i (on i<limit edge)
            JitInstr::Return { src: 2 }, // 2
            JitInstr::Return { src: 1 }, // 3: else
        ],
    );
    let iv = interval_analysis(&prog);
    // On the body edge i is refined to [MIN, limit.hi-1] = [MIN, MAX-1]; the
    // header in-set (ip 0) still sees i as TOP (param, untracked).
    assert_eq!(iv[0][1], Interval::TOP);
    assert_eq!(iv[1][1].hi, i64::MAX as i128 - 1);

    // Unreachable edge: `if i < i` — the taken edge asserts the empty fact i < i.
    // The two per-operand narrowings (i.hi <= i.hi-1, i.lo >= i.lo+1) each apply
    // to the SAME register but never invert it in a single `apply`, so the result
    // is a sound (over-approximating) but WELL-FORMED interval. The contract this
    // test locks is soundness's structural half: NO malformed interval (lo > hi)
    // ever escapes the refinement, even on an unreachable edge.
    let bad = f(
        1,
        2,
        vec![
            JitInstr::JumpIfIntCompare {
                lhs: 0,
                rhs: 0,
                op: JitCompare::Lt,
                expected: true,
                target: 2,
            },
            JitInstr::Return { src: 0 }, // 1: fall-through (i >= i, always)
            JitInstr::Return { src: 0 }, // 2: taken (i < i, unreachable)
        ],
    );
    let iv2 = interval_analysis(&bad);
    // Every register interval at every ip is well-formed (lo <= hi).
    for row in &iv2 {
        for v in row {
            assert!(v.lo <= v.hi, "malformed interval {v:?}");
        }
    }
}

fn constant_mod_access(
    list_ty: JitValueType,
    result_ty: JitValueType,
    set_value: Option<(JitValueType, i64)>,
) -> JitFunction {
    let mut reg_types = vec![
        list_ty,
        JitValueType::Int,
        JitValueType::Int,
        JitValueType::Int,
        result_ty,
    ];
    let mut code = vec![
        JitInstr::LoadInt { dst: 1, value: 11 },
        JitInstr::LoadInt { dst: 2, value: 4 },
        JitInstr::Mod {
            dst: 3,
            lhs: 1,
            rhs: 2,
        },
    ];
    match (list_ty, set_value) {
        (JitValueType::FlatInt | JitValueType::FlatIntMut, None) => {
            code.push(JitInstr::ListGetIntDirect {
                dst: 4,
                base: 0,
                index: 3,
            });
        }
        (JitValueType::FlatFloat | JitValueType::FlatFloatMut, None) => {
            code.push(JitInstr::ListGetFloatDirect {
                dst: 4,
                base: 0,
                index: 3,
            });
        }
        (JitValueType::FlatIntMut, Some((value_ty, bits))) => {
            reg_types.push(value_ty);
            code.push(JitInstr::LoadInt {
                dst: 5,
                value: bits,
            });
            code.push(JitInstr::ListSetIntDirect {
                dst: 4,
                base: 0,
                index: 3,
                value: 5,
            });
        }
        _ => unreachable!("unsupported test list operation"),
    }
    code.push(JitInstr::Return { src: 4 });
    ft(1, reg_types, code)
}

#[test]
fn mod_interval_transfer_tracks_result_sign_and_magnitude() {
    let positive = f(
        0,
        3,
        vec![
            JitInstr::LoadInt { dst: 0, value: 17 },
            JitInstr::LoadInt { dst: 1, value: 5 },
            JitInstr::Mod {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            JitInstr::Return { src: 2 },
        ],
    );
    let positive_intervals = interval_analysis(&positive);
    assert_eq!(positive_intervals[3][2], Interval { lo: 0, hi: 4 });

    let negative = f(
        0,
        3,
        vec![
            JitInstr::LoadInt { dst: 0, value: -17 },
            JitInstr::LoadInt { dst: 1, value: 5 },
            JitInstr::Mod {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            JitInstr::Return { src: 2 },
        ],
    );
    assert_eq!(
        interval_analysis(&negative)[3][2],
        Interval { lo: -4, hi: 0 }
    );
}

#[test]
fn list_bounds_plan_accepts_only_unique_sound_provenance() {
    use JitValueType::{FlatInt, Int};

    let constant = constant_mod_access(FlatInt, Int, None);
    let constant_plan = list_bounds_plan(&constant, &interval_analysis(&constant), false);
    assert_eq!(constant_plan.unchecked_ips, [3].into_iter().collect());
    assert_eq!(constant_plan.entry_min_len.get(&0), Some(&4));

    // Unique Move forwarding is provenance-preserving for both divisor and
    // modulo result.
    let moved = ft(
        1,
        vec![FlatInt, Int, Int, Int, Int, Int, Int],
        vec![
            JitInstr::LoadInt { dst: 1, value: 9 },
            JitInstr::LoadInt { dst: 2, value: 3 },
            JitInstr::Move { dst: 3, src: 2 },
            JitInstr::Mod {
                dst: 4,
                lhs: 1,
                rhs: 3,
            },
            JitInstr::Move { dst: 5, src: 4 },
            JitInstr::ListGetIntDirect {
                dst: 6,
                base: 0,
                index: 5,
            },
            JitInstr::Return { src: 6 },
        ],
    );
    assert!(
        list_bounds_plan(&moved, &interval_analysis(&moved), false)
            .unchecked_ips
            .contains(&5)
    );

    // A second reachable definition makes the divisor ambiguous even though
    // the last definition is also positive.
    let multi_def = ft(
        1,
        vec![FlatInt, Int, Int, Int, Int],
        vec![
            JitInstr::LoadInt { dst: 1, value: 9 },
            JitInstr::LoadInt { dst: 2, value: 3 },
            JitInstr::LoadInt { dst: 2, value: 4 },
            JitInstr::Mod {
                dst: 3,
                lhs: 1,
                rhs: 2,
            },
            JitInstr::ListGetIntDirect {
                dst: 4,
                base: 0,
                index: 3,
            },
            JitInstr::Return { src: 4 },
        ],
    );
    assert!(
        list_bounds_plan(&multi_def, &interval_analysis(&multi_def), false)
            .unchecked_ips
            .is_empty()
    );

    // A parameter's incoming value is its first definition. Overwriting it
    // once in code is still multi-def and cannot manufacture provenance.
    let overwritten_param = ft(
        3,
        vec![FlatInt, Int, Int, Int, Int],
        vec![
            JitInstr::LoadInt { dst: 1, value: 9 },
            JitInstr::LoadInt { dst: 2, value: 4 },
            JitInstr::Mod {
                dst: 3,
                lhs: 1,
                rhs: 2,
            },
            JitInstr::ListGetIntDirect {
                dst: 4,
                base: 0,
                index: 3,
            },
            JitInstr::Return { src: 4 },
        ],
    );
    assert!(
        list_bounds_plan(
            &overwritten_param,
            &interval_analysis(&overwritten_param),
            false,
        )
        .unchecked_ips
        .is_empty()
    );
}

#[test]
fn constant_modulo_elides_all_direct_get_and_set_checks() {
    use JitValueType::{FlatFloat, FlatFloatMut, FlatInt, FlatIntMut, Float, Int};
    let mut m = module();

    let int_get = m.compile(&constant_mod_access(FlatInt, Int, None)).unwrap();
    let ints = [10, 20, 30, 40];
    assert_eq!(
        m.call_with_host_ctx(
            int_get,
            &[ints.as_ptr() as i64],
            &[ints.len() as i64],
            0,
            &mut [FlatBufferArg::Int(&ints)],
        )
        .completed(),
        Some(40)
    );

    let float_get = m
        .compile(&constant_mod_access(FlatFloat, Float, None))
        .unwrap();
    let floats = [1.0, 2.0, 3.0, 4.5];
    let bits = m
        .call_with_host_ctx(
            float_get,
            &[floats.as_ptr() as i64],
            &[floats.len() as i64],
            0,
            &mut [FlatBufferArg::Float(&floats)],
        )
        .completed()
        .unwrap();
    assert_eq!(f64::from_bits(bits as u64), 4.5);

    let int_set = m
        .compile(&constant_mod_access(FlatIntMut, Int, Some((Int, 99))))
        .unwrap();
    let mut mutable = [10, 20, 30, 40];
    let mutable_ptr = mutable.as_mut_ptr() as i64;
    assert_eq!(
        m.call_with_host_ctx(
            int_set,
            &[mutable_ptr],
            &[mutable.len() as i64],
            0,
            &mut [FlatBufferArg::IntMut(&mut mutable)],
        )
        .completed(),
        Some(0)
    );
    assert_eq!(mutable, [10, 20, 30, 99]);

    let float_set_program = ft(
        1,
        vec![FlatFloatMut, Int, Int, Int, Float, Int],
        vec![
            JitInstr::LoadInt { dst: 1, value: 11 },
            JitInstr::LoadInt { dst: 2, value: 4 },
            JitInstr::Mod {
                dst: 3,
                lhs: 1,
                rhs: 2,
            },
            JitInstr::LoadFloat {
                dst: 4,
                value: 9.25,
            },
            JitInstr::ListSetFloatDirect {
                dst: 5,
                base: 0,
                index: 3,
                value: 4,
            },
            JitInstr::Return { src: 5 },
        ],
    );
    assert!(
        list_bounds_plan(
            &float_set_program,
            &interval_analysis(&float_set_program),
            false,
        )
        .unchecked_ips
        .contains(&4)
    );
    let float_set = m.compile(&float_set_program).unwrap();
    let mut mutable_floats = [1.0, 2.0, 3.0, 4.0];
    let mutable_floats_ptr = mutable_floats.as_mut_ptr() as i64;
    assert_eq!(
        m.call_with_host_ctx(
            float_set,
            &[mutable_floats_ptr],
            &[mutable_floats.len() as i64],
            0,
            &mut [FlatBufferArg::FloatMut(&mut mutable_floats)],
        )
        .completed(),
        Some(0)
    );
    assert_eq!(mutable_floats, [1.0, 2.0, 3.0, 9.25]);
}

#[test]
fn constant_modulo_short_list_deopts_anonymously_before_source() {
    use JitValueType::{FlatInt, Int};
    let mut m = module();
    let program = constant_mod_access(FlatInt, Int, None);
    let id = m.compile(&program).unwrap();
    // Only the two checked-Mod sites remain. The direct access at ip 3 has no
    // safepoint, while its entry length guard is intentionally anonymous.
    let map = m.deopt_map(id).unwrap();
    assert_eq!(map.sites.len(), 2);
    assert!(map.sites.iter().all(|site| site.resume_ip == 2));

    let short = [10, 20, 30];
    assert!(matches!(
        m.call_with_host_ctx(
            id,
            &[short.as_ptr() as i64],
            &[short.len() as i64],
            0,
            &mut [FlatBufferArg::Int(&short)],
        ),
        NativeOutcome::Deopt {
            safepoint_id: SafepointId::ANONYMOUS,
            live,
            ..
        } if live.is_empty()
    ));
}

#[test]
fn same_base_list_len_modulo_elides_access_but_preserves_mod_deopt_ip() {
    use JitValueType::{FlatInt, Int};
    let program = ft(
        1,
        vec![FlatInt, Int, Int, Int, Int],
        vec![
            JitInstr::ListLenDirect { dst: 1, base: 0 },
            JitInstr::LoadInt { dst: 2, value: 7 },
            JitInstr::Mod {
                dst: 3,
                lhs: 2,
                rhs: 1,
            },
            JitInstr::ListGetIntDirect {
                dst: 4,
                base: 0,
                index: 3,
            },
            JitInstr::Return { src: 4 },
        ],
    );
    let plan = list_bounds_plan(&program, &interval_analysis(&program), false);
    assert_eq!(plan.unchecked_ips, [3].into_iter().collect());
    assert!(plan.entry_min_len.is_empty());

    let mut m = module();
    let id = m.compile(&program).unwrap();
    assert_eq!(
        m.direct_list_bounds_checks_elided(id),
        Some(1),
        "compiled metadata must expose the range proof as evidence"
    );
    assert_eq!(m.deopt_map(id).unwrap().sites.len(), 2);
    let empty: [i64; 0] = [];
    match m.call_with_host_ctx(
        id,
        &[empty.as_ptr() as i64],
        &[0],
        0,
        &mut [FlatBufferArg::Int(&empty)],
    ) {
        NativeOutcome::Deopt {
            safepoint_id, live, ..
        } => {
            assert_eq!(safepoint_id, SafepointId(1));
            assert_eq!(m.deopt_map(id).unwrap().sites[0].resume_ip, 2);
            assert!(live.iter().any(|value| value.reg == 1));
        }
        outcome => panic!("expected modulo deopt, got {outcome:?}"),
    }
}

#[test]
fn negative_and_wrong_base_modulo_accesses_stay_checked() {
    use JitValueType::{FlatInt, Int};
    let negative = ft(
        1,
        vec![FlatInt, Int, Int, Int, Int],
        vec![
            JitInstr::LoadInt { dst: 1, value: -1 },
            JitInstr::LoadInt { dst: 2, value: 4 },
            JitInstr::Mod {
                dst: 3,
                lhs: 1,
                rhs: 2,
            },
            JitInstr::ListGetIntDirect {
                dst: 4,
                base: 0,
                index: 3,
            },
            JitInstr::Return { src: 4 },
        ],
    );
    assert!(
        list_bounds_plan(&negative, &interval_analysis(&negative), false)
            .unchecked_ips
            .is_empty()
    );

    let wrong_base = ft(
        2,
        vec![FlatInt, FlatInt, Int, Int, Int, Int],
        vec![
            JitInstr::ListLenDirect { dst: 2, base: 0 },
            JitInstr::LoadInt { dst: 3, value: 7 },
            JitInstr::Mod {
                dst: 4,
                lhs: 3,
                rhs: 2,
            },
            JitInstr::ListGetIntDirect {
                dst: 5,
                base: 1,
                index: 4,
            },
            JitInstr::Return { src: 5 },
        ],
    );
    assert!(
        list_bounds_plan(&wrong_base, &interval_analysis(&wrong_base), false)
            .unchecked_ips
            .is_empty()
    );

    let mut m = module();
    let negative_id = m.compile(&negative).unwrap();
    let values = [10, 20, 30, 40];
    match m.call_with_host_ctx(
        negative_id,
        &[values.as_ptr() as i64],
        &[values.len() as i64],
        0,
        &mut [FlatBufferArg::Int(&values)],
    ) {
        NativeOutcome::Deopt { safepoint_id, .. } => {
            let site = &m.deopt_map(negative_id).unwrap().sites[safepoint_id.0 as usize - 1];
            assert_eq!(site.resume_ip, 3);
        }
        outcome => panic!("expected checked negative-index deopt, got {outcome:?}"),
    }

    let wrong_base_id = m.compile(&wrong_base).unwrap();
    let divisor_list = [10, 20, 30, 40];
    let target_list = [99];
    match m.call_with_host_ctx(
        wrong_base_id,
        &[divisor_list.as_ptr() as i64, target_list.as_ptr() as i64],
        &[divisor_list.len() as i64, target_list.len() as i64],
        0,
        &mut [
            FlatBufferArg::Int(&divisor_list),
            FlatBufferArg::Int(&target_list),
        ],
    ) {
        NativeOutcome::Deopt { safepoint_id, .. } => {
            let site = &m.deopt_map(wrong_base_id).unwrap().sites[safepoint_id.0 as usize - 1];
            assert_eq!(site.resume_ip, 3);
        }
        outcome => panic!("expected checked wrong-base deopt, got {outcome:?}"),
    }
}
use super::*;
