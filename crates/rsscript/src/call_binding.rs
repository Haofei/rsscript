#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundArgumentSource {
    Receiver,
    Explicit(usize),
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoundArgument {
    pub(crate) parameter_index: usize,
    pub(crate) evaluation_index: usize,
    pub(crate) source: BoundArgumentSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallBindingIssue {
    UnknownNamedArgument {
        source_index: usize,
    },
    DuplicateParameter {
        source_index: usize,
        parameter_index: usize,
    },
    PositionalArgumentOutOfRange {
        source_index: usize,
    },
    MissingParameter {
        parameter_index: usize,
    },
}

/// Canonical binding between source arguments and declaration-order parameters.
///
/// Explicit arguments keep source evaluation order. Defaults follow explicit
/// arguments in declaration order. `by_parameter` is the ABI layout consumed by
/// lowering backends; `evaluation_order` is the language evaluation contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallBinding {
    by_parameter: Vec<Option<BoundArgument>>,
    evaluation_order: Vec<BoundArgument>,
    issues: Vec<CallBindingIssue>,
}

impl CallBinding {
    pub(crate) fn bind(
        parameter_names: &[impl AsRef<str>],
        parameter_has_default: &[bool],
        parameter_allows_shorthand: &[bool],
        argument_names: &[Option<&str>],
        argument_shorthand_names: &[Option<&str>],
        receiver_offset: usize,
    ) -> Self {
        debug_assert_eq!(parameter_names.len(), parameter_has_default.len());
        debug_assert_eq!(parameter_names.len(), parameter_allows_shorthand.len());
        debug_assert_eq!(argument_names.len(), argument_shorthand_names.len());
        debug_assert!(receiver_offset <= parameter_names.len());

        let mut by_parameter = vec![None; parameter_names.len()];
        let mut evaluation_order = Vec::with_capacity(parameter_names.len());
        let mut issues = Vec::new();

        if receiver_offset == 1 && !parameter_names.is_empty() {
            let receiver = BoundArgument {
                parameter_index: 0,
                evaluation_index: 0,
                source: BoundArgumentSource::Receiver,
            };
            by_parameter[0] = Some(receiver);
            evaluation_order.push(receiver);
        }

        for (source_index, argument_name) in argument_names.iter().enumerate() {
            let shorthand_name = argument_shorthand_names[source_index].and_then(|candidate| {
                parameter_names
                    .iter()
                    .enumerate()
                    .find(|(index, parameter)| {
                        parameter_allows_shorthand[*index] && parameter.as_ref() == candidate
                    })
                    .map(|(_, parameter)| parameter.as_ref())
            });
            let effective_name = (*argument_name).or(shorthand_name);
            let parameter_index = if let Some(argument_name) = effective_name {
                let Some(parameter_index) = parameter_names
                    .iter()
                    .position(|parameter| parameter.as_ref() == argument_name)
                else {
                    issues.push(CallBindingIssue::UnknownNamedArgument { source_index });
                    continue;
                };
                parameter_index
            } else {
                source_index + receiver_offset
            };

            if parameter_index >= parameter_names.len() {
                issues.push(CallBindingIssue::PositionalArgumentOutOfRange { source_index });
                continue;
            }
            if by_parameter[parameter_index].is_some() {
                issues.push(CallBindingIssue::DuplicateParameter {
                    source_index,
                    parameter_index,
                });
                continue;
            }

            let argument = BoundArgument {
                parameter_index,
                evaluation_index: source_index + receiver_offset,
                source: BoundArgumentSource::Explicit(source_index),
            };
            by_parameter[parameter_index] = Some(argument);
            evaluation_order.push(argument);
        }

        let mut next_evaluation = argument_names.len() + receiver_offset;
        for parameter_index in receiver_offset..parameter_names.len() {
            if by_parameter[parameter_index].is_some() {
                continue;
            }
            if parameter_has_default[parameter_index] {
                let argument = BoundArgument {
                    parameter_index,
                    evaluation_index: next_evaluation,
                    source: BoundArgumentSource::Default,
                };
                next_evaluation += 1;
                by_parameter[parameter_index] = Some(argument);
                evaluation_order.push(argument);
            } else {
                issues.push(CallBindingIssue::MissingParameter { parameter_index });
            }
        }

        evaluation_order.sort_by_key(|argument| argument.evaluation_index);
        Self {
            by_parameter,
            evaluation_order,
            issues,
        }
    }

    pub(crate) fn explicit(&self, source_index: usize) -> Option<BoundArgument> {
        self.evaluation_order
            .iter()
            .copied()
            .find(|argument| argument.source == BoundArgumentSource::Explicit(source_index))
    }

    pub(crate) fn defaults(&self) -> impl Iterator<Item = BoundArgument> + '_ {
        self.evaluation_order
            .iter()
            .copied()
            .filter(|argument| argument.source == BoundArgumentSource::Default)
    }

    #[cfg(test)]
    pub(crate) fn by_parameter(&self) -> &[Option<BoundArgument>] {
        &self.by_parameter
    }

    pub(crate) fn evaluation_order(&self) -> &[BoundArgument] {
        &self.evaluation_order
    }

    #[cfg(test)]
    pub(crate) fn issues(&self) -> &[CallBindingIssue] {
        &self.issues
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.issues.is_empty() && self.by_parameter.iter().all(Option::is_some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_arguments_preserve_evaluation_order_and_bind_by_parameter() {
        let binding = CallBinding::bind(
            &["a", "b", "c"],
            &[true, false, true],
            &[true, true, true],
            &[Some("c"), Some("b")],
            &[None, None],
            0,
        );

        assert!(binding.is_complete());
        assert_eq!(
            binding.evaluation_order(),
            &[
                BoundArgument {
                    parameter_index: 2,
                    evaluation_index: 0,
                    source: BoundArgumentSource::Explicit(0),
                },
                BoundArgument {
                    parameter_index: 1,
                    evaluation_index: 1,
                    source: BoundArgumentSource::Explicit(1),
                },
                BoundArgument {
                    parameter_index: 0,
                    evaluation_index: 2,
                    source: BoundArgumentSource::Default,
                },
            ]
        );
        assert_eq!(
            binding
                .by_parameter()
                .iter()
                .map(|argument| argument.unwrap().source)
                .collect::<Vec<_>>(),
            vec![
                BoundArgumentSource::Default,
                BoundArgumentSource::Explicit(1),
                BoundArgumentSource::Explicit(0),
            ]
        );
    }

    #[test]
    fn receiver_occupies_parameter_zero_without_reordering_explicit_arguments() {
        let binding = CallBinding::bind(
            &["self", "source", "target"],
            &[false, false, false],
            &[true, true, true],
            &[Some("target"), Some("source")],
            &[None, None],
            1,
        );

        assert!(binding.is_complete());
        assert_eq!(
            binding
                .evaluation_order()
                .iter()
                .map(|argument| (argument.parameter_index, argument.evaluation_index))
                .collect::<Vec<_>>(),
            vec![(0, 0), (2, 1), (1, 2)]
        );
    }

    #[test]
    fn malformed_bindings_fail_closed_without_overwriting_slots() {
        let binding = CallBinding::bind(
            &["a", "b"],
            &[false, false],
            &[true, true],
            &[Some("a"), Some("a"), Some("unknown")],
            &[None, None, None],
            0,
        );

        assert!(!binding.is_complete());
        assert_eq!(binding.issues().len(), 3);
        assert_eq!(
            binding.by_parameter()[0].unwrap().source,
            BoundArgumentSource::Explicit(0)
        );
        assert!(binding.by_parameter()[1].is_none());
    }

    #[test]
    fn same_name_shorthand_uses_the_named_slot_in_any_source_order() {
        let binding = CallBinding::bind(
            &["first", "second"],
            &[false, false],
            &[true, true],
            &[None, None],
            &[Some("second"), Some("first")],
            0,
        );

        assert!(binding.is_complete());
        assert_eq!(binding.explicit(0).unwrap().parameter_index, 1);
        assert_eq!(binding.explicit(1).unwrap().parameter_index, 0);
    }
}
