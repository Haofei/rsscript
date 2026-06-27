use super::expr::*;
use super::scan::*;
use super::stmt::*;
use super::types::*;
use super::*;

pub(super) struct ParsedFields {
    pub(super) fields: Vec<FieldDecl>,
    pub(super) malformed_spans: Vec<crate::diagnostic::Span>,
}

pub(super) struct ParsedParams {
    pub(super) params: Vec<Param>,
    pub(super) malformed_spans: Vec<crate::diagnostic::Span>,
}

pub(super) struct ParsedGenericParams {
    pub(super) params: Vec<GenericParam>,
    pub(super) malformed_spans: Vec<crate::diagnostic::Span>,
}

pub(super) struct ParsedEffects {
    pub(super) effects: Vec<EffectDecl>,
    pub(super) malformed_spans: Vec<crate::diagnostic::Span>,
}

pub(super) struct ParamRange {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) empty_span: Option<crate::diagnostic::Span>,
}

pub(super) fn parse_fields(tokens: &[Token], start: usize, end: usize) -> ParsedFields {
    let mut fields = Vec::new();
    let mut malformed_spans = Vec::new();
    let mut index = start;
    while index < end {
        if is_trivia_boundary(&tokens[index]) {
            index += 1;
            continue;
        }
        if tokens[index].is_ident_text("drop") {
            index = skip_braced_block(tokens, index).unwrap_or(end);
            continue;
        }

        let name_index = index;
        let line_end = next_line_or_block_end(tokens, index, end);
        if let Some(name) = tokens.get(name_index).and_then(ident_name)
            && tokens
                .get(name_index + 1)
                .is_some_and(|token| token.symbol(":"))
        {
            let mut ty_start = name_index + 2;
            let mut is_handle = false;
            let mut is_weak = false;
            while let Some(token) = tokens.get(ty_start) {
                if token.is_ident_text("handle") {
                    is_handle = true;
                    ty_start += 1;
                } else if token.is_ident_text("weak") {
                    is_weak = true;
                    ty_start += 1;
                } else {
                    break;
                }
            }
            let line_limit = next_line_or_block_end(tokens, ty_start, end);
            // A default value: `name: Type = <expr>`. The type ends at the `=`.
            let default_eq = (ty_start..line_limit).find(|&i| tokens[i].symbol("="));
            let ty_end = default_eq.unwrap_or(line_limit);
            let default = default_eq.and_then(|eq| parse_expr(tokens, eq + 1, line_limit));
            if let Some(ty) = parse_type_ref(tokens, ty_start, ty_end) {
                fields.push(FieldDecl {
                    name: name.to_string(),
                    ty,
                    is_handle,
                    is_weak,
                    default,
                    span: tokens[name_index].span.clone(),
                });
            } else {
                malformed_spans.push(tokens[name_index].span.clone());
            }
        } else {
            malformed_spans.push(tokens[name_index].span.clone());
        }

        index = line_end.max(index + 1);
    }
    ParsedFields {
        fields,
        malformed_spans,
    }
}

pub(super) fn parse_drop_body(tokens: &[Token], start: usize, end: usize) -> Option<Block> {
    let drop_index = (start..end).find(|index| tokens[*index].is_ident_text("drop"))?;
    let open = (drop_index + 1..end).find(|index| tokens[*index].symbol("{"))?;
    let close = find_matching(tokens, open, "{", "}")?;
    (close <= end).then(|| parse_block(tokens, open, close))
}

pub(super) fn parse_params(tokens: &[Token], start: usize, end: usize) -> ParsedParams {
    let mut params = Vec::new();
    let mut malformed_spans = Vec::new();
    for range in split_param_ranges(tokens, start, end) {
        if let Some(span) = range.empty_span {
            malformed_spans.push(span);
            continue;
        }
        let start = range.start;
        let end = range.end;
        let Some(name) = tokens.get(start).and_then(ident_name) else {
            if let Some(token) = tokens.get(start) {
                malformed_spans.push(token.span.clone());
            }
            continue;
        };
        if parse_data_effect(tokens.get(start)).is_some() {
            malformed_spans.push(tokens[start].span.clone());
            continue;
        }

        if !tokens.get(start + 1).is_some_and(|token| token.symbol(":")) {
            params.push(Param {
                name: name.to_string(),
                effect: None,
                ty: TypeRef {
                    name: String::new(),
                    args: Vec::new(),
                    malformed_arg_spans: Vec::new(),
                    is_fresh: false,
                    is_noescape: false,
                    is_owned: false,
                    fn_params: Vec::new(),
                    fn_param_effects: Vec::new(),
                    fn_return: None,
                    span: tokens[start].span.clone(),
                },
                default: None,
                span: tokens[start].span.clone(),
            });
            continue;
        }

        let mut ty_start = start + 2;
        let effect = parse_data_effect(tokens.get(ty_start)).inspect(|_| {
            ty_start += 1;
        });
        // A default value: `name: Type = <expr>`. The type ends at the `=`.
        let default_eq = (ty_start..end).find(|&i| tokens[i].symbol("="));
        let ty_end = default_eq.unwrap_or(end);
        let default = default_eq.and_then(|eq| parse_expr(tokens, eq + 1, end));
        let ty = parse_type_ref(tokens, ty_start, ty_end).unwrap_or_else(|| TypeRef {
            name: String::new(),
            args: Vec::new(),
            malformed_arg_spans: Vec::new(),
            is_fresh: false,
            is_noescape: false,
            is_owned: false,
            fn_params: Vec::new(),
            fn_param_effects: Vec::new(),
            fn_return: None,
            span: tokens[start].span.clone(),
        });
        params.push(Param {
            name: name.to_string(),
            effect,
            ty,
            default,
            span: tokens[start].span.clone(),
        });
    }

    ParsedParams {
        params,
        malformed_spans,
    }
}

pub(super) fn split_param_ranges(tokens: &[Token], start: usize, end: usize) -> Vec<ParamRange> {
    let mut ranges = Vec::new();
    let mut range_start = start;
    let mut depth = 0usize;
    let mut pipe_depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if depth == 0 && token.symbol("|") {
            pipe_depth = 1usize.saturating_sub(pipe_depth);
        } else if pipe_depth == 0
            && (token.symbol("(") || token.symbol("{") || token.symbol("[") || token.symbol("<"))
        {
            depth += 1;
        } else if pipe_depth == 0
            && (token.symbol(")") || token.symbol("}") || token.symbol("]") || token.symbol(">"))
        {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && pipe_depth == 0 && token.symbol(",") {
            if range_start < index {
                ranges.push(ParamRange {
                    start: range_start,
                    end: index,
                    empty_span: None,
                });
            } else {
                ranges.push(ParamRange {
                    start: index,
                    end: index,
                    empty_span: Some(token.span.clone()),
                });
            }
            range_start = index + 1;
        }
    }
    if range_start < end {
        ranges.push(ParamRange {
            start: range_start,
            end,
            empty_span: None,
        });
    }
    ranges
}

pub(super) fn parse_effects(tokens: &[Token], start: usize, end: usize) -> ParsedEffects {
    let mut effects = Vec::new();
    let mut malformed_spans = Vec::new();
    for range in split_param_ranges(tokens, start, end) {
        if let Some(span) = range.empty_span {
            malformed_spans.push(span);
            continue;
        }
        let start = range.start;
        let end = range.end;
        let Some(name) = tokens.get(start).and_then(ident_name) else {
            if let Some(token) = tokens.get(start) {
                malformed_spans.push(token.span.clone());
            }
            continue;
        };
        if name == "retains" {
            if tokens.get(start + 1).is_some_and(|token| token.symbol("("))
                && let Some(close) = find_matching(tokens, start + 1, "(", ")")
                && close + 1 == end
                && start + 3 == close
                && let Some(param) = tokens.get(start + 2).and_then(ident_name)
            {
                effects.push(EffectDecl::Retains(param.to_string()));
            } else {
                malformed_spans.push(tokens[start].span.clone());
            }
        } else if start + 1 == end {
            effects.push(EffectDecl::Name(name.to_string()));
        } else {
            malformed_spans.push(tokens[start].span.clone());
        }
    }
    ParsedEffects {
        effects,
        malformed_spans,
    }
}

pub(super) fn parse_generic_params(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> ParsedGenericParams {
    let mut params = Vec::new();
    let mut malformed_spans = Vec::new();
    for range in split_param_ranges(tokens, start, end) {
        if let Some(span) = range.empty_span {
            malformed_spans.push(span);
            continue;
        }
        let start = range.start;
        let end = range.end;
        let Some(name) = tokens.get(start).and_then(ident_name) else {
            if let Some(token) = tokens.get(start) {
                malformed_spans.push(token.span.clone());
            }
            continue;
        };

        let bound = if start + 1 == end {
            None
        } else if tokens.get(start + 1).is_some_and(|token| token.symbol(":")) && start + 3 == end {
            let Some(bound) = tokens.get(start + 2).and_then(parse_generic_bound) else {
                malformed_spans.push(tokens[start].span.clone());
                continue;
            };
            Some(bound)
        } else {
            malformed_spans.push(tokens[start].span.clone());
            continue;
        };

        params.push(GenericParam {
            name: name.to_string(),
            bound,
            span: tokens[start].span.clone(),
        });
    }
    ParsedGenericParams {
        params,
        malformed_spans,
    }
}

fn parse_generic_bound(token: &Token) -> Option<GenericBound> {
    if token.is_ident_text("Managed") {
        Some(GenericBound::Managed)
    } else if token.is_ident_text("Struct") {
        Some(GenericBound::Struct)
    } else if token.is_ident_text("Resource") {
        Some(GenericBound::Resource)
    } else {
        ident_name(token).map(|name| GenericBound::Protocol(name.to_string()))
    }
}
