use super::items::*;
use super::scan::*;
use super::*;

/// A tuple type `(T, U, ...)` desugars to the synthetic generic struct
/// `__TupleN<T, U, ...>`. `(T)` is grouping, not a tuple.
fn parse_tuple_type_ref(tokens: &[Token], start: usize, end: usize) -> Option<TypeRef> {
    if !tokens.get(start)?.symbol("(") || !tokens.get(end.checked_sub(1)?)?.symbol(")") {
        return None;
    }
    let close = find_matching(tokens, start, "(", ")")?;
    if close + 1 != end {
        return None;
    }
    let ranges: Vec<_> = split_param_ranges(tokens, start + 1, close)
        .into_iter()
        .filter(|range| range.empty_span.is_none())
        .collect();
    if ranges.len() < 2 {
        return None;
    }
    let args: Vec<TypeRef> = ranges
        .iter()
        .filter_map(|range| parse_type_ref(tokens, range.start, range.end))
        .collect();
    if args.len() != ranges.len() {
        return None;
    }
    Some(TypeRef {
        name: format!("__Tuple{}", ranges.len()),
        args,
        malformed_arg_spans: Vec::new(),
        is_fresh: false,
        is_noescape: false,
        is_owned: false,
        fn_params: Vec::new(),
        fn_param_effects: Vec::new(),
        fn_return: None,
        span: tokens[start].span.clone(),
    })
}

pub(super) fn parse_type_ref(tokens: &[Token], start: usize, end: usize) -> Option<TypeRef> {
    if let Some(tuple) = parse_tuple_type_ref(tokens, start, end) {
        return Some(tuple);
    }
    // `capability <Protocol>` (spec §20.2-2) is sugar for `Capability<<Protocol>>`:
    // an explicit, review-visible dynamic-dispatch boundary. It may follow an
    // effect keyword (`read capability Store<T>`); the effect is handled by the
    // parameter parser, so skip it here when locating the `capability` marker.
    if let Some(cap_index) = (start..end).find(|index| {
        ident_name(&tokens[*index])
            .is_some_and(|name| name != "read" && name != "mut" && name != "take")
    }) && ident_name(&tokens[cap_index]).is_some_and(|name| name == "capability")
    {
        let inner = parse_type_ref(tokens, cap_index + 1, end)?;
        return Some(TypeRef {
            name: "Capability".to_string(),
            args: vec![inner],
            malformed_arg_spans: Vec::new(),
            is_fresh: false,
            is_noescape: false,
            is_owned: false,
            fn_params: Vec::new(),
            fn_param_effects: Vec::new(),
            fn_return: None,
            span: tokens[cap_index].span.clone(),
        });
    }
    let is_fresh = tokens
        .get(start)
        .and_then(ident_name)
        .is_some_and(|name| name == "fresh");
    let is_noescape = tokens
        .get(start)
        .and_then(ident_name)
        .is_some_and(|name| name == "noescape");
    let is_owned = tokens
        .get(start)
        .and_then(ident_name)
        .is_some_and(|name| name == "owned");
    let name_index = (start..end).find(|index| {
        ident_name(&tokens[*index]).is_some_and(|name| {
            !matches!(
                name,
                "read" | "mut" | "take" | "fresh" | "handle" | "weak" | "noescape" | "owned"
            )
        })
    })?;
    let (name, name_end) = parse_type_name(tokens, name_index, end)?;
    let mut args = Vec::new();
    let mut malformed_arg_spans = Vec::new();
    let mut cursor = name_end;
    if tokens.get(cursor).is_some_and(|token| token.symbol("<")) {
        if let Some(close) = find_matching(tokens, cursor, "<", ">") {
            for range in split_param_ranges(tokens, cursor + 1, close) {
                if let Some(span) = range.empty_span {
                    malformed_arg_spans.push(span);
                    continue;
                }
                let Some(arg) = parse_type_ref(tokens, range.start, range.end) else {
                    if let Some(token) = tokens.get(range.start) {
                        malformed_arg_spans.push(token.span.clone());
                    }
                    continue;
                };
                args.push(arg);
            }
            cursor = close + 1;
        } else {
            malformed_arg_spans.push(tokens[cursor].span.clone());
        }
    }
    let mut fn_params = Vec::new();
    let mut fn_param_effects = Vec::new();
    if name == "Fn"
        && tokens.get(cursor).is_some_and(|token| token.symbol("("))
        && let Some(close) = find_matching(tokens, cursor, "(", ")")
    {
        for range in split_param_ranges(tokens, cursor + 1, close) {
            if let Some(span) = range.empty_span {
                malformed_arg_spans.push(span);
                continue;
            }
            // A `Fn(...)` parameter may carry a leading data effect
            // (`read`/`mut`/`take`), exactly like a regular function parameter.
            // Capture it positionally (parallel to `fn_params`) so the checker,
            // VM and AOT lowerer can honor it; `parse_type_ref` strips the
            // keyword while parsing the bare parameter type.
            let effect = tokens
                .get(range.start)
                .and_then(ident_name)
                .and_then(|n| match n {
                    "read" => Some(DataEffect::Read),
                    "mut" => Some(DataEffect::Mut),
                    "take" => Some(DataEffect::Take),
                    _ => None,
                });
            let Some(param) = parse_type_ref(tokens, range.start, range.end) else {
                if let Some(token) = tokens.get(range.start) {
                    malformed_arg_spans.push(token.span.clone());
                }
                continue;
            };
            fn_params.push(param);
            fn_param_effects.push(effect);
        }
        cursor = close + 1;
    }
    let fn_return = if name == "Fn" && tokens.get(cursor).is_some_and(|token| token.symbol("->")) {
        parse_type_ref(tokens, cursor + 1, end).map(Box::new)
    } else {
        None
    };
    Some(TypeRef {
        name,
        args,
        malformed_arg_spans,
        is_fresh,
        is_noescape,
        is_owned,
        fn_params,
        fn_param_effects,
        fn_return,
        span: tokens[name_index].span.clone(),
    })
}

fn parse_type_name(tokens: &[Token], start: usize, end: usize) -> Option<(String, usize)> {
    let mut index = start;
    let mut name = ident_name(tokens.get(index)?)?.to_string();
    index += 1;
    while index + 1 < end
        && tokens.get(index).is_some_and(|token| token.symbol("."))
        && tokens.get(index + 1).and_then(ident_name).is_some()
    {
        name.push('.');
        name.push_str(ident_name(&tokens[index + 1])?);
        index += 2;
    }
    Some((name, index))
}

pub(super) fn type_ref_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let prefix = ty
                    .effective_fn_param_effect(index)
                    .map(|effect| format!("{} ", effect.as_str()))
                    .unwrap_or_default();
                format!("{prefix}{}", type_ref_name(param))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_ref_name(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        let args = ty
            .args
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{args}>", ty.name)
    };
    let name = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
}

pub(super) fn parse_data_effect(token: Option<&Token>) -> Option<DataEffect> {
    let token = token?;
    if token.is_ident_text("read") {
        Some(DataEffect::Read)
    } else if token.is_ident_text("mut") {
        Some(DataEffect::Mut)
    } else if token.is_ident_text("take") {
        Some(DataEffect::Take)
    } else {
        None
    }
}

pub(super) fn parse_file_feature(token: Option<&Token>) -> Option<FileFeature> {
    let token = token?;
    if token.is_ident_text("local") {
        Some(FileFeature::Local)
    } else if token.is_ident_text("native") {
        Some(FileFeature::Native)
    } else if token.is_ident_text("unsafe") {
        Some(FileFeature::Unsafe)
    } else if token.is_ident_text("async") {
        Some(FileFeature::Async)
    } else if token.is_ident_text("device") {
        Some(FileFeature::Device)
    } else if token.is_ident_text("ffi") {
        Some(FileFeature::Ffi)
    } else if token.is_ident_text("reflection") {
        Some(FileFeature::Reflection)
    } else {
        None
    }
}

pub(super) fn file_feature_name(feature: FileFeature) -> &'static str {
    match feature {
        FileFeature::Local => "local",
        FileFeature::Native => "native",
        FileFeature::Unsafe => "unsafe",
        FileFeature::Async => "async",
        FileFeature::Device => "device",
        FileFeature::Ffi => "ffi",
        FileFeature::Reflection => "reflection",
    }
}
