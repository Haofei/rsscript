#[cfg(test)]
mod intrinsic_registry_tests {
    use super::super::*;

    /// The conservative DEFAULT must hold for an intrinsic no site special-cases:
    /// opaque allocator, not foldable, not native-lowerable, no combinator/string
    /// role, no cold-arm whitelist. This locks the table's contract for lever 2.
    #[test]
    fn default_is_conservative() {
        // `ListContains` is a representative unlisted intrinsic (allocating, opaque).
        let d = intrinsic_descriptor(RegIntrinsic::ListContains);
        assert_eq!(d.effect, IntrinsicEffect::Allocate);
        assert!(!d.can_fold);
        assert!(!d.native_lowerable);
        assert!(!d.view_capable);
        assert!(d.combinator_kind.is_none());
        assert!(d.string_fold_role.is_none());
        assert!(d.bytes_fold_role.is_none());
        assert!(!d.cold_arm_pure_builder);

        // The bare `Default` impl matches the conservative classification.
        let def = IntrinsicDescriptor::default();
        assert_eq!(def.effect, IntrinsicEffect::Allocate);
        assert!(!def.can_fold);
        assert!(!def.native_lowerable);
        assert!(!def.view_capable);
    }

    /// `view_capable` is a reserved placeholder — false for EVERY intrinsic today.
    #[test]
    fn view_capable_is_false_everywhere() {
        for i in [
            RegIntrinsic::IntToFloat,
            RegIntrinsic::OptionMap,
            RegIntrinsic::ResultAndThen,
            RegIntrinsic::StringLen,
            RegIntrinsic::StringSlice,
            RegIntrinsic::StringFromInt,
            RegIntrinsic::ListContains,
        ] {
            assert!(!intrinsic_descriptor(i).view_capable);
        }
    }

    /// Site 1: native-lowerable intrinsics are explicit opt-ins.
    #[test]
    fn native_lowerable_intrinsics_are_explicit() {
        for i in [
            RegIntrinsic::IntToFloat,
            RegIntrinsic::StringFromInt,
            RegIntrinsic::StringLen,
            RegIntrinsic::StringSlice,
            RegIntrinsic::StringPadLeft,
            RegIntrinsic::StringSplit,
            RegIntrinsic::StringStartsWith,
            RegIntrinsic::BytesLen,
            RegIntrinsic::BytesSlice,
        ] {
            assert!(intrinsic_descriptor(i).native_lowerable);
        }
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::IntToFloat).effect,
            IntrinsicEffect::Pure
        );
        // A non-listed intrinsic is NOT native-lowerable.
        assert!(!intrinsic_descriptor(RegIntrinsic::ListContains).native_lowerable);
    }

    /// Site 2: exactly the six Option/Result combinators are expandable, each with
    /// its kind; nothing else is.
    #[test]
    fn six_combinators_are_expandable() {
        let cases = [
            (RegIntrinsic::OptionMap, CombinatorKind::OptionMap),
            (RegIntrinsic::OptionAndThen, CombinatorKind::OptionAndThen),
            (RegIntrinsic::OptionUnwrapOr, CombinatorKind::OptionUnwrapOr),
            (RegIntrinsic::ResultMap, CombinatorKind::ResultMap),
            (RegIntrinsic::ResultAndThen, CombinatorKind::ResultAndThen),
            (RegIntrinsic::ResultUnwrapOr, CombinatorKind::ResultUnwrapOr),
        ];
        for (i, k) in cases {
            let d = intrinsic_descriptor(i);
            assert_eq!(d.combinator_kind, Some(k));
            assert!(d.can_fold);
            assert_eq!(d.effect, IntrinsicEffect::Pure);
        }
        assert!(
            intrinsic_descriptor(RegIntrinsic::StringLen)
                .combinator_kind
                .is_none()
        );
        assert!(
            intrinsic_descriptor(RegIntrinsic::ListContains)
                .combinator_kind
                .is_none()
        );
    }

    /// Site 3: the string-fold producers/query carry the expected roles.
    #[test]
    fn string_fold_roles_match() {
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::StringLen).string_fold_role,
            Some(StringFoldRole::LengthQuery)
        );
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::StringFromInt).string_fold_role,
            Some(StringFoldRole::ProducerFromInt)
        );
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::StringSlice).string_fold_role,
            Some(StringFoldRole::ProducerSlice)
        );
        for i in [
            RegIntrinsic::StringLen,
            RegIntrinsic::StringFromInt,
            RegIntrinsic::StringSlice,
        ] {
            assert!(intrinsic_descriptor(i).can_fold);
        }
        // A non-fold string intrinsic has no role.
        assert!(
            intrinsic_descriptor(RegIntrinsic::StringCopy)
                .string_fold_role
                .is_none()
        );
    }

    /// Site 3 (Bytes sibling): the Bytes-fold producers/query carry the expected roles
    /// and are `can_fold`; an unrelated Bytes intrinsic has no Bytes role.
    #[test]
    fn bytes_fold_roles_match() {
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesLen).bytes_fold_role,
            Some(BytesFoldRole::LengthQuery)
        );
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesFromString).bytes_fold_role,
            Some(BytesFoldRole::ProducerFromString)
        );
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesSlice).bytes_fold_role,
            Some(BytesFoldRole::ProducerSlice)
        );
        for i in [
            RegIntrinsic::BytesLen,
            RegIntrinsic::BytesFromString,
            RegIntrinsic::BytesSlice,
        ] {
            assert!(intrinsic_descriptor(i).can_fold);
            // Bytes-fold intrinsics carry NO string role (they are a disjoint family).
            assert!(intrinsic_descriptor(i).string_fold_role.is_none());
        }
        // `Bytes.len` is a pure READ; the producers allocate.
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesLen).effect,
            IntrinsicEffect::Read
        );
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesFromString).effect,
            IntrinsicEffect::Allocate
        );
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesSlice).effect,
            IntrinsicEffect::Allocate
        );
        // A non-fold Bytes intrinsic has no Bytes role.
        assert!(
            intrinsic_descriptor(RegIntrinsic::BytesConcat)
                .bytes_fold_role
                .is_none()
        );
        // String-fold intrinsics carry no Bytes role.
        assert!(
            intrinsic_descriptor(RegIntrinsic::StringLen)
                .bytes_fold_role
                .is_none()
        );
    }

    /// The deopt cold-arm pure-builder whitelist: the four String builders plus the
    /// pure slice/pad/bytes Allocate producers. Each reads only its operands and is
    /// faithfully re-runnable on the interpreter after a native cold-arm `Bail`.
    #[test]
    fn cold_arm_pure_builders_whitelist() {
        for i in [
            RegIntrinsic::StringCopy,
            RegIntrinsic::StringFromBool,
            RegIntrinsic::StringFromFloat,
            RegIntrinsic::StringFromInt,
            RegIntrinsic::StringSlice,
            RegIntrinsic::StringPadLeft,
            RegIntrinsic::BytesFromString,
            RegIntrinsic::BytesSlice,
        ] {
            assert!(intrinsic_descriptor(i).cold_arm_pure_builder, "{:?}", i);
        }
        // Not on the builder whitelist: queries, combinators, opaque allocators / mutators.
        for i in [
            RegIntrinsic::StringLen,
            RegIntrinsic::OptionMap,
            RegIntrinsic::ListContains,
        ] {
            assert!(!intrinsic_descriptor(i).cold_arm_pure_builder, "{:?}", i);
        }
        // Pure first-order scalar READERS are cold-arm eligible via the separate
        // `cold_arm_pure_reader` flag (NOT builders — they allocate nothing).
        for i in [
            RegIntrinsic::StringCount,
            RegIntrinsic::StringContains,
            RegIntrinsic::StringIndexOf,
            RegIntrinsic::StringStartsWith,
        ] {
            let d = intrinsic_descriptor(i);
            assert!(d.cold_arm_pure_reader, "reader {:?}", i);
            assert!(!d.cold_arm_pure_builder, "reader is not a builder {:?}", i);
        }
        // Higher-order combinators carry neither the builder nor the reader flag (they are
        // not value producers/queries); they are cold-arm eligible via `combinator_kind`
        // instead — a combinator + its closure run only on the interpreter replay after a
        // cold-arm bail, so any closure effect happens exactly as without the JIT.
        for i in [RegIntrinsic::OptionMap, RegIntrinsic::ResultAndThen] {
            let d = intrinsic_descriptor(i);
            assert!(
                !d.cold_arm_pure_reader && !d.cold_arm_pure_builder,
                "{:?}",
                i
            );
            assert!(d.combinator_kind.is_some(), "combinator {:?}", i);
        }
    }
}

