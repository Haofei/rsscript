//! MIR verification pass (free functions), split out of `lib.rs` for
//! module-size partitioning. Behavior-preserving; visibility raised to
//! pub(super) so `MirModule::verify` in the crate root can still call them.

use super::*;

pub(super) fn verify_instruction_sources(
    function: &MirFunction,
    debug: &MirFunctionDebug,
) -> Result<(), MirValidationError> {
    let mut seen = BTreeSet::new();
    for entry in debug.instruction_sources() {
        let Some(block) = function.blocks().get(entry.block().index()) else {
            return Err(MirValidationError::InvalidInstructionSourceBlock {
                function: function.id(),
                block: entry.block(),
            });
        };
        if block.id() != entry.block() {
            return Err(MirValidationError::InvalidInstructionSourceBlock {
                function: function.id(),
                block: entry.block(),
            });
        }
        if entry.instruction_index() as usize >= block.instructions().len() {
            return Err(MirValidationError::InvalidInstructionSourceIndex {
                function: function.id(),
                block: entry.block(),
                instruction_index: entry.instruction_index(),
            });
        }
        if !seen.insert((entry.block(), entry.instruction_index())) {
            return Err(MirValidationError::DuplicateInstructionSource {
                function: function.id(),
                block: entry.block(),
                instruction_index: entry.instruction_index(),
            });
        }
    }
    Ok(())
}

pub(super) fn verify_type_layouts(
    types: &[WireType],
    layouts: &[MirTypeLayout],
) -> Result<(), MirValidationError> {
    let mut names = BTreeSet::new();
    let mut layout_types = BTreeSet::new();
    for layout in layouts {
        if layout.name.is_empty() || !names.insert(layout.name.clone()) {
            return Err(MirValidationError::InvalidTypeLayout {
                ty: layout.ty,
                name: layout.name.clone(),
            });
        }
        if !layout_types.insert(layout.ty)
            || !matches!(
                types.get(layout.ty.index()),
                Some(WireType::Named { name, .. }) if name == &layout.name
            )
        {
            return Err(MirValidationError::InvalidTypeLayout {
                ty: layout.ty,
                name: layout.name.clone(),
            });
        }
        let mut fields = BTreeSet::new();
        for (name, ty) in &layout.fields {
            if name.is_empty() || !fields.insert(name) || ty.index() >= types.len() {
                return Err(MirValidationError::InvalidTypeLayout {
                    ty: layout.ty,
                    name: layout.name.clone(),
                });
            }
        }
    }
    Ok(())
}

pub(super) fn verify_variant_layouts(
    types: &[WireType],
    layouts: &[MirVariantLayout],
) -> Result<(), MirValidationError> {
    let mut names = BTreeSet::new();
    let mut layout_types = BTreeSet::new();
    for layout in layouts {
        if layout.name.is_empty() || !names.insert(layout.name.clone()) {
            return Err(MirValidationError::InvalidTypeLayout {
                ty: layout.ty,
                name: layout.name.clone(),
            });
        }
        if !layout_types.insert(layout.ty)
            || !matches!(
                types.get(layout.ty.index()),
                Some(WireType::Named { name, .. }) if name == &layout.name
            )
        {
            return Err(MirValidationError::InvalidTypeLayout {
                ty: layout.ty,
                name: layout.name.clone(),
            });
        }
        let mut variant_names = BTreeSet::new();
        for variant in &layout.variants {
            if variant.name.is_empty() || !variant_names.insert(&variant.name) {
                return Err(MirValidationError::InvalidTypeLayout {
                    ty: layout.ty,
                    name: layout.name.clone(),
                });
            }
            let mut field_names = BTreeSet::new();
            for (field, ty) in &variant.fields {
                if field.is_empty() || !field_names.insert(field) || ty.index() >= types.len() {
                    return Err(MirValidationError::InvalidTypeLayout {
                        ty: layout.ty,
                        name: layout.name.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

pub(super) fn verify_resource_types(
    function: &MirFunction,
    types: &[WireType],
) -> Result<(), MirValidationError> {
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let MirInstruction::AcquireResource { resource_type, .. } = instruction {
                match types.get(resource_type.index()) {
                    Some(WireType::Resource { .. }) => {}
                    _ => {
                        return Err(MirValidationError::InvalidResourceType {
                            function: function.id,
                            resource_type: *resource_type,
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

pub(super) fn verify_record_types(
    function: &MirFunction,
    types: &[WireType],
) -> Result<(), MirValidationError> {
    for block in function.blocks() {
        for instruction in block.instructions() {
            match instruction {
                MirInstruction::MakeStruct { ty, fields, .. }
                | MirInstruction::MakeVariant { ty, fields, .. } => {
                    if !matches!(types.get(ty.index()), Some(WireType::Named { .. })) {
                        return Err(MirValidationError::InvalidRecordType {
                            function: function.id,
                            ty: *ty,
                        });
                    }
                    let mut names = BTreeSet::new();
                    for (field, _) in fields {
                        if field.is_empty() || !names.insert(field) {
                            return Err(MirValidationError::InvalidAggregateField {
                                function: function.id,
                                field: field.clone(),
                            });
                        }
                    }
                }
                MirInstruction::GetField { field, .. } if field.is_empty() => {
                    return Err(MirValidationError::InvalidAggregateField {
                        function: function.id,
                        field: field.clone(),
                    });
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Track resources independently from ordinary move state: an acquired
/// resource must be released on every reachable return path. Joining paths is
/// conservative (a resource live on any predecessor is live at the join).
pub(super) fn verify_resource_lifetimes(function: &MirFunction) -> Result<(), MirValidationError> {
    let mut entries = vec![BTreeSet::new(); function.blocks.len()];
    let mut queued = vec![false; function.blocks.len()];
    let mut visited = vec![false; function.blocks.len()];
    let mut worklist = VecDeque::from([BlockId::new(0)]);
    queued[0] = true;
    while let Some(block_id) = worklist.pop_front() {
        queued[block_id.index()] = false;
        visited[block_id.index()] = true;
        let block = &function.blocks[block_id.index()];
        let mut live = entries[block_id.index()].clone();
        for instruction in block.instructions() {
            match instruction {
                MirInstruction::AcquireResource { place, .. } if !live.insert(*place) => {
                    return Err(MirValidationError::ResourceAlreadyLive {
                        function: function.id,
                        place: *place,
                    });
                }
                MirInstruction::ReleaseResource { place } if !live.remove(place) => {
                    return Err(MirValidationError::ResourceNotLive {
                        function: function.id,
                        place: *place,
                    });
                }
                _ => {}
            }
        }
        if matches!(block.terminator(), MirTerminator::Return(_)) && !live.is_empty() {
            return Err(MirValidationError::ResourceLeak {
                function: function.id,
                place: *live.iter().next().expect("non-empty resource set"),
            });
        }
        for successor in successors(block.terminator()) {
            let entry = &mut entries[successor.index()];
            let before = entry.len();
            entry.extend(live.iter().copied());
            if (!visited[successor.index()] || entry.len() != before) && !queued[successor.index()]
            {
                worklist.push_back(successor);
                queued[successor.index()] = true;
            }
        }
    }
    Ok(())
}

/// Child tasks are lexically owned. A task cannot silently escape a return
/// edge: it must have been awaited, cancelled, or joined with its task group.
pub(super) fn verify_task_lifetimes(function: &MirFunction) -> Result<(), MirValidationError> {
    let mut spawn_sites = BTreeSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let MirInstruction::Spawn { task, .. } = instruction
                && !spawn_sites.insert(*task)
            {
                return Err(MirValidationError::DuplicateTaskId {
                    function: function.id,
                    task: *task,
                });
            }
        }
    }

    let mut entries = vec![BTreeMap::<TaskId, TaskGroupId>::new(); function.blocks.len()];
    let mut queued = vec![false; function.blocks.len()];
    let mut visited = vec![false; function.blocks.len()];
    let mut worklist = VecDeque::from([BlockId::new(0)]);
    queued[0] = true;
    while let Some(block_id) = worklist.pop_front() {
        queued[block_id.index()] = false;
        visited[block_id.index()] = true;
        let block = &function.blocks[block_id.index()];
        let mut live = entries[block_id.index()].clone();
        for instruction in block.instructions() {
            match instruction {
                MirInstruction::Spawn { task, group, .. }
                    if live.insert(*task, *group).is_some() =>
                {
                    return Err(MirValidationError::TaskAlreadyLive {
                        function: function.id,
                        task: *task,
                    });
                }
                MirInstruction::Await { task, .. } | MirInstruction::Cancel { task }
                    if live.remove(task).is_none() =>
                {
                    return Err(MirValidationError::TaskNotLive {
                        function: function.id,
                        task: *task,
                    });
                }
                // A first-ready selection transfers the winning result to
                // ordinary values and cancels/reaps every losing child before
                // arm dispatch. It therefore closes all selected task
                // lifetimes at this explicit boundary.
                MirInstruction::Select { tasks, .. } => {
                    for task in tasks {
                        if live.remove(task).is_none() {
                            return Err(MirValidationError::TaskNotLive {
                                function: function.id,
                                task: *task,
                            });
                        }
                    }
                }
                MirInstruction::Join { group } => live.retain(|_, owner| owner != group),
                _ => {}
            }
        }
        if matches!(block.terminator(), MirTerminator::Return(_))
            && let Some((task, _)) = live.iter().next()
        {
            return Err(MirValidationError::TaskLeak {
                function: function.id,
                task: *task,
            });
        }
        for successor in successors(block.terminator()) {
            let entry = &mut entries[successor.index()];
            let mut changed = false;
            for (task, group) in &live {
                match entry.get(task) {
                    Some(existing) if existing != group => {
                        return Err(MirValidationError::TaskGroupMismatch {
                            function: function.id,
                            task: *task,
                        });
                    }
                    Some(_) => {}
                    None => {
                        entry.insert(*task, *group);
                        changed = true;
                    }
                }
            }
            if (!visited[successor.index()] || changed) && !queued[successor.index()] {
                worklist.push_back(successor);
                queued[successor.index()] = true;
            }
        }
    }
    Ok(())
}

pub(super) fn verify_function(
    function: &MirFunction,
    type_count: usize,
    functions: &[MirFunction],
    external_imports: &[MirExternalImport],
) -> Result<(), MirValidationError> {
    if function.blocks.is_empty() {
        return Err(MirValidationError::EmptyFunction {
            function: function.id,
        });
    }
    for ty in function
        .signature
        .parameter_types()
        .iter()
        .copied()
        .chain(std::iter::once(function.signature.result()))
    {
        if ty.index() >= type_count {
            return Err(MirValidationError::InvalidType {
                function: function.id,
                ty,
            });
        }
    }
    if function.signature.parameter_types().len() != function.signature.parameter_modes().len() {
        return Err(MirValidationError::FunctionParameterModeCount {
            function: function.id,
            types: function.signature.parameter_types().len(),
            modes: function.signature.parameter_modes().len(),
        });
    }
    if (function.place_count as usize)
        < function.captures.len() + function.signature.parameter_types().len()
    {
        return Err(MirValidationError::ClosureFrameTooSmall {
            function: function.id,
            required: function.captures.len() + function.signature.parameter_types().len(),
            actual: function.place_count as usize,
        });
    }
    for capture in function.captures() {
        if capture.ty().index() >= type_count {
            return Err(MirValidationError::InvalidClosureCaptureType {
                function: function.id,
                ty: capture.ty(),
            });
        }
    }

    let mut defined = BTreeSet::new();
    let mut used = Vec::new();
    for (index, block) in function.blocks.iter().enumerate() {
        if block.id.index() != index {
            return Err(MirValidationError::BlockIdMismatch {
                function: function.id,
                expected: index,
                actual: block.id.index(),
            });
        }
        let mut block_moved_places = BTreeSet::new();
        for instruction in &block.instructions {
            verify_instruction(
                function,
                instruction,
                InstructionVerification {
                    defined: &mut defined,
                    used: &mut used,
                    moved_places: &mut block_moved_places,
                    type_count,
                    functions,
                    external_imports,
                },
            )?;
        }
        verify_terminator(function, block.terminator(), &mut used)?;
    }
    verify_move_dataflow(function)?;
    verify_value_dominance(function)?;
    for value in used {
        if value.index() >= function.value_count as usize || !defined.contains(&value) {
            return Err(MirValidationError::UndefinedValue {
                function: function.id,
                value,
            });
        }
    }
    Ok(())
}

/// Every value use must be reached by a definition on every control-flow path.
/// MIR has no phi instruction yet, so a value defined in only one branch cannot
/// be consumed after that branch joins. This catches a class of malformed CFGs
/// that a whole-function "defined somewhere" set cannot distinguish.
pub(super) fn verify_value_dominance(function: &MirFunction) -> Result<(), MirValidationError> {
    let mut entries = vec![None::<BTreeSet<ValueId>>; function.blocks.len()];
    entries[0] = Some(BTreeSet::new());
    let mut changed = true;
    while changed {
        changed = false;
        for block in &function.blocks {
            let Some(entry) = entries[block.id.index()].clone() else {
                continue;
            };
            let mut exit = entry;
            for instruction in &block.instructions {
                for destination in instruction_definitions(instruction) {
                    exit.insert(destination);
                }
            }
            for successor in successors(block.terminator()) {
                let slot = &mut entries[successor.index()];
                let merged = match slot {
                    Some(existing) => existing.intersection(&exit).copied().collect(),
                    None => exit.clone(),
                };
                if slot.as_ref() != Some(&merged) {
                    *slot = Some(merged);
                    changed = true;
                }
            }
        }
    }

    for block in &function.blocks {
        let Some(mut defined) = entries[block.id.index()].clone() else {
            continue;
        };
        for instruction in &block.instructions {
            for value in instruction_uses(instruction) {
                if !defined.contains(&value) {
                    return Err(MirValidationError::ValueDoesNotDominate {
                        function: function.id,
                        block: block.id,
                        value,
                    });
                }
            }
            for destination in instruction_definitions(instruction) {
                defined.insert(destination);
            }
        }
        for value in terminator_uses(block.terminator()) {
            if !defined.contains(&value) {
                return Err(MirValidationError::ValueDoesNotDominate {
                    function: function.id,
                    block: block.id,
                    value,
                });
            }
        }
    }
    Ok(())
}

pub(super) fn instruction_definitions(instruction: &MirInstruction) -> Vec<ValueId> {
    match instruction {
        MirInstruction::LoadLiteral { destination, .. }
        | MirInstruction::MakeList { destination, .. }
        | MirInstruction::MakeMap { destination, .. }
        | MirInstruction::MakeObject { destination, .. }
        | MirInstruction::MakeStruct { destination, .. }
        | MirInstruction::MakeVariant { destination, .. }
        | MirInstruction::MakeResult { destination, .. }
        | MirInstruction::UnwrapResult { destination, .. }
        | MirInstruction::MakeOption { destination, .. }
        | MirInstruction::UnwrapOption { destination, .. }
        | MirInstruction::ListGet { destination, .. }
        | MirInstruction::ListAppend { destination, .. }
        | MirInstruction::ListClear { destination, .. }
        | MirInstruction::ListPop { destination, .. }
        | MirInstruction::ListPush { destination, .. }
        | MirInstruction::ListRemoveAt { destination, .. }
        | MirInstruction::ListSet { destination, .. }
        | MirInstruction::SetClear { destination, .. }
        | MirInstruction::SetInsert { destination, .. }
        | MirInstruction::SetRemove { destination, .. }
        | MirInstruction::DequeClear { destination, .. }
        | MirInstruction::DequePopBack { destination, .. }
        | MirInstruction::DequePopFront { destination, .. }
        | MirInstruction::DequePushBack { destination, .. }
        | MirInstruction::DequePushFront { destination, .. }
        | MirInstruction::SortedMapClear { destination, .. }
        | MirInstruction::SortedMapInsert { destination, .. }
        | MirInstruction::SortedMapRemove { destination, .. }
        | MirInstruction::SortedSetClear { destination, .. }
        | MirInstruction::SortedSetInsert { destination, .. }
        | MirInstruction::SortedSetRemove { destination, .. }
        | MirInstruction::BufferClear { destination, .. }
        | MirInstruction::StringBuilderPush { destination, .. }
        | MirInstruction::StringBuilderFinish { destination, .. }
        | MirInstruction::MapGet { destination, .. }
        | MirInstruction::MapClear { destination, .. }
        | MirInstruction::MapInsert { destination, .. }
        | MirInstruction::MapInsertOld { destination, .. }
        | MirInstruction::MapRemove { destination, .. }
        | MirInstruction::GetField { destination, .. }
        | MirInstruction::ListLen { destination, .. }
        | MirInstruction::ReadPlace { destination, .. }
        | MirInstruction::BorrowRead { destination, .. }
        | MirInstruction::TakePlace { destination, .. }
        | MirInstruction::Manage { destination, .. }
        | MirInstruction::Binary { destination, .. }
        | MirInstruction::StringConcat { destination, .. }
        | MirInstruction::Call { destination, .. }
        | MirInstruction::MakeClosure { destination, .. }
        | MirInstruction::CallClosure { destination, .. }
        | MirInstruction::Await { destination, .. }
        | MirInstruction::TryResult { destination, .. } => vec![*destination],
        MirInstruction::Select { winner, value, .. } => vec![*winner, *value],
        MirInstruction::WritePlace { .. }
        | MirInstruction::Retain { .. }
        | MirInstruction::Drop { .. }
        | MirInstruction::AcquireResource { .. }
        | MirInstruction::ReleaseResource { .. }
        | MirInstruction::Spawn { .. }
        | MirInstruction::Cancel { .. }
        | MirInstruction::Join { .. }
        | MirInstruction::SetField { .. }
        | MirInstruction::Discard { .. } => Vec::new(),
    }
}

pub(super) fn instruction_uses(instruction: &MirInstruction) -> Vec<ValueId> {
    match instruction {
        MirInstruction::WritePlace { value, .. } | MirInstruction::Discard { value } => {
            vec![*value]
        }
        MirInstruction::MakeList { items, .. } => items.clone(),
        MirInstruction::MakeMap { entries, .. } => entries
            .iter()
            .flat_map(|(key, value)| [*key, *value])
            .collect(),
        MirInstruction::MakeObject { fields, .. } => {
            fields.iter().map(|(_, value)| *value).collect()
        }
        MirInstruction::MakeStruct { fields, .. } => {
            fields.iter().map(|(_, value)| *value).collect()
        }
        MirInstruction::MakeVariant { fields, .. } => {
            fields.iter().map(|(_, value)| *value).collect()
        }
        MirInstruction::MakeResult { value, .. } => vec![*value],
        MirInstruction::UnwrapResult { source, .. } => vec![*source],
        MirInstruction::MakeOption { value, .. } => value.iter().copied().collect(),
        MirInstruction::UnwrapOption { source, .. } => vec![*source],
        MirInstruction::ListGet { list, index, .. } => vec![*list, *index],
        MirInstruction::ListAppend { values, .. }
        | MirInstruction::ListPush { value: values, .. } => {
            vec![*values]
        }
        MirInstruction::ListRemoveAt { index, .. } => vec![*index],
        MirInstruction::ListSet { index, value, .. } => vec![*index, *value],
        MirInstruction::SetInsert { value, .. } | MirInstruction::SetRemove { value, .. } => {
            vec![*value]
        }
        MirInstruction::DequePushBack { value, .. }
        | MirInstruction::DequePushFront { value, .. } => vec![*value],
        MirInstruction::SortedMapInsert { key, value, .. } => vec![*key, *value],
        MirInstruction::SortedMapRemove { key, .. } => vec![*key],
        MirInstruction::SortedSetInsert { value, .. }
        | MirInstruction::SortedSetRemove { value, .. } => vec![*value],
        MirInstruction::StringBuilderPush { value, .. }
        | MirInstruction::StringBuilderFinish { builder: value, .. } => vec![*value],
        MirInstruction::MapGet { map, key, .. } => vec![*map, *key],
        MirInstruction::MapInsert { key, value, .. }
        | MirInstruction::MapInsertOld { key, value, .. } => vec![*key, *value],
        MirInstruction::MapRemove { key, .. } => vec![*key],
        MirInstruction::GetField { base, .. } => vec![*base],
        MirInstruction::SetField { base, value, .. } => vec![*base, *value],
        MirInstruction::ListLen { list, .. } => vec![*list],
        MirInstruction::AcquireResource { source, .. } => vec![*source],
        MirInstruction::Manage { source, .. } => vec![*source],
        MirInstruction::Binary { left, right, .. }
        | MirInstruction::StringConcat { left, right, .. } => vec![*left, *right],
        MirInstruction::Call { arguments, .. } => arguments
            .iter()
            .filter_map(|argument| match argument {
                MirCallArgument::Value(value) => Some(*value),
                MirCallArgument::BorrowRead(_)
                | MirCallArgument::BorrowMut(_)
                | MirCallArgument::Take(_) => None,
            })
            .collect(),
        MirInstruction::MakeClosure { captures, .. } => captures
            .iter()
            .filter_map(|capture| match capture {
                MirCallArgument::Value(value) => Some(*value),
                MirCallArgument::BorrowRead(_)
                | MirCallArgument::BorrowMut(_)
                | MirCallArgument::Take(_) => None,
            })
            .collect(),
        MirInstruction::CallClosure {
            closure, arguments, ..
        } => std::iter::once(*closure)
            .chain(arguments.iter().filter_map(|argument| match argument {
                MirCallArgument::Value(value) => Some(*value),
                MirCallArgument::BorrowRead(_)
                | MirCallArgument::BorrowMut(_)
                | MirCallArgument::Take(_) => None,
            }))
            .collect(),
        MirInstruction::Spawn { arguments, .. } => arguments
            .iter()
            .filter_map(|argument| match argument {
                MirCallArgument::Value(value) => Some(*value),
                MirCallArgument::BorrowRead(_)
                | MirCallArgument::BorrowMut(_)
                | MirCallArgument::Take(_) => None,
            })
            .collect(),
        MirInstruction::LoadLiteral { .. }
        | MirInstruction::ReadPlace { .. }
        | MirInstruction::BorrowRead { .. }
        | MirInstruction::TakePlace { .. }
        | MirInstruction::Retain { .. }
        | MirInstruction::Drop { .. }
        | MirInstruction::ReleaseResource { .. }
        | MirInstruction::Await { .. }
        | MirInstruction::Select { .. }
        | MirInstruction::Cancel { .. }
        | MirInstruction::Join { .. }
        | MirInstruction::MapClear { .. }
        | MirInstruction::ListClear { .. }
        | MirInstruction::ListPop { .. }
        | MirInstruction::SetClear { .. }
        | MirInstruction::DequeClear { .. }
        | MirInstruction::DequePopBack { .. }
        | MirInstruction::DequePopFront { .. }
        | MirInstruction::SortedMapClear { .. }
        | MirInstruction::SortedSetClear { .. }
        | MirInstruction::BufferClear { .. } => Vec::new(),
        MirInstruction::TryResult { source, .. } => vec![*source],
    }
}

pub(super) fn terminator_uses(terminator: &MirTerminator) -> Vec<ValueId> {
    match terminator {
        MirTerminator::Return(Some(value)) => vec![*value],
        MirTerminator::Branch { condition, .. } => vec![*condition],
        MirTerminator::MatchVariant { value, .. } => vec![*value],
        MirTerminator::MatchResult { value, .. } => vec![*value],
        MirTerminator::MatchOption { value, .. } => vec![*value],
        MirTerminator::Return(None) | MirTerminator::Jump(_) | MirTerminator::Unreachable => {
            Vec::new()
        }
    }
}

/// A place is considered moved at a join when any reachable predecessor moves
/// it. This is deliberately conservative: a later read must be valid on every
/// control-flow path. Assigning the place reinitializes it on that path.
pub(super) fn verify_move_dataflow(function: &MirFunction) -> Result<(), MirValidationError> {
    let mut entries = vec![BTreeSet::new(); function.blocks.len()];
    let mut queued = vec![false; function.blocks.len()];
    let mut visited = vec![false; function.blocks.len()];
    let mut worklist = VecDeque::from([BlockId::new(0)]);
    queued[0] = true;

    while let Some(block_id) = worklist.pop_front() {
        queued[block_id.index()] = false;
        visited[block_id.index()] = true;
        let block = &function.blocks[block_id.index()];
        let mut moved_places = entries[block_id.index()].clone();
        for instruction in &block.instructions {
            transfer_move_state(function, instruction, &mut moved_places)?;
        }
        for successor in successors(block.terminator()) {
            let entry = &mut entries[successor.index()];
            let before = entry.len();
            entry.extend(moved_places.iter().copied());
            if (!visited[successor.index()] || entry.len() != before) && !queued[successor.index()]
            {
                worklist.push_back(successor);
                queued[successor.index()] = true;
            }
        }
    }
    Ok(())
}

pub(super) fn successors(terminator: &MirTerminator) -> impl Iterator<Item = BlockId> {
    let mut successors = [None; 2];
    match terminator {
        MirTerminator::Jump(target) => successors[0] = Some(*target),
        MirTerminator::Branch {
            then_target,
            else_target,
            ..
        } => {
            successors[0] = Some(*then_target);
            successors[1] = Some(*else_target);
        }
        MirTerminator::MatchVariant {
            match_target,
            else_target,
            ..
        } => {
            successors[0] = Some(*match_target);
            successors[1] = Some(*else_target);
        }
        MirTerminator::MatchResult {
            ok_target,
            err_target,
            ..
        } => {
            successors[0] = Some(*ok_target);
            successors[1] = Some(*err_target);
        }
        MirTerminator::MatchOption {
            some_target,
            none_target,
            ..
        } => {
            successors[0] = Some(*some_target);
            successors[1] = Some(*none_target);
        }
        MirTerminator::Return(_) | MirTerminator::Unreachable => {}
    }
    successors.into_iter().flatten()
}

pub(super) fn transfer_move_state(
    function: &MirFunction,
    instruction: &MirInstruction,
    moved_places: &mut BTreeSet<PlaceId>,
) -> Result<(), MirValidationError> {
    let check_live = |place: PlaceId, moved_places: &BTreeSet<PlaceId>| {
        if moved_places.contains(&place) {
            Err(MirValidationError::UseAfterMove {
                function: function.id,
                place,
            })
        } else {
            Ok(())
        }
    };
    match instruction {
        MirInstruction::ReadPlace { place, .. } | MirInstruction::BorrowRead { place, .. } => {
            check_live(*place, moved_places)
        }
        MirInstruction::TakePlace { place, .. } => {
            check_live(*place, moved_places)?;
            moved_places.insert(*place);
            Ok(())
        }
        MirInstruction::Manage { .. } => Ok(()),
        MirInstruction::Retain { place } => check_live(*place, moved_places),
        MirInstruction::Drop { place } => {
            check_live(*place, moved_places)?;
            moved_places.insert(*place);
            Ok(())
        }
        MirInstruction::AcquireResource { place, .. } => {
            moved_places.remove(place);
            Ok(())
        }
        MirInstruction::ReleaseResource { place } => {
            check_live(*place, moved_places)?;
            moved_places.insert(*place);
            Ok(())
        }
        MirInstruction::WritePlace { place, .. } => {
            moved_places.remove(place);
            Ok(())
        }
        MirInstruction::MapClear { map, .. } => check_live(*map, moved_places),
        MirInstruction::ListAppend { list, .. }
        | MirInstruction::ListClear { list, .. }
        | MirInstruction::ListPop { list, .. }
        | MirInstruction::ListPush { list, .. }
        | MirInstruction::ListRemoveAt { list, .. }
        | MirInstruction::ListSet { list, .. } => check_live(*list, moved_places),
        MirInstruction::SetClear { set, .. }
        | MirInstruction::SetInsert { set, .. }
        | MirInstruction::SetRemove { set, .. } => check_live(*set, moved_places),
        MirInstruction::DequeClear { deque, .. }
        | MirInstruction::DequePopBack { deque, .. }
        | MirInstruction::DequePopFront { deque, .. }
        | MirInstruction::DequePushBack { deque, .. }
        | MirInstruction::DequePushFront { deque, .. } => check_live(*deque, moved_places),
        MirInstruction::SortedMapClear { map, .. }
        | MirInstruction::SortedMapInsert { map, .. }
        | MirInstruction::SortedMapRemove { map, .. } => check_live(*map, moved_places),
        MirInstruction::SortedSetClear { set, .. }
        | MirInstruction::SortedSetInsert { set, .. }
        | MirInstruction::SortedSetRemove { set, .. } => check_live(*set, moved_places),
        MirInstruction::BufferClear { buffer, .. }
        | MirInstruction::StringBuilderPush {
            builder: buffer, ..
        } => check_live(*buffer, moved_places),
        MirInstruction::MapInsert { map, .. }
        | MirInstruction::MapInsertOld { map, .. }
        | MirInstruction::MapRemove { map, .. } => check_live(*map, moved_places),
        MirInstruction::Call { arguments, .. }
        | MirInstruction::MakeClosure {
            captures: arguments,
            ..
        }
        | MirInstruction::CallClosure { arguments, .. } => {
            for argument in arguments {
                match argument {
                    MirCallArgument::Value(_) => {}
                    MirCallArgument::BorrowRead(place) | MirCallArgument::BorrowMut(place) => {
                        check_live(*place, moved_places)?;
                    }
                    MirCallArgument::Take(place) => {
                        check_live(*place, moved_places)?;
                        moved_places.insert(*place);
                    }
                }
            }
            Ok(())
        }
        MirInstruction::Spawn { arguments, .. } => {
            for argument in arguments {
                match argument {
                    MirCallArgument::Value(_) => {}
                    MirCallArgument::BorrowRead(place) | MirCallArgument::BorrowMut(place) => {
                        check_live(*place, moved_places)?;
                    }
                    MirCallArgument::Take(place) => {
                        check_live(*place, moved_places)?;
                        moved_places.insert(*place);
                    }
                }
            }
            Ok(())
        }
        MirInstruction::LoadLiteral { .. }
        | MirInstruction::MakeList { .. }
        | MirInstruction::MakeMap { .. }
        | MirInstruction::MakeObject { .. }
        | MirInstruction::MakeStruct { .. }
        | MirInstruction::MakeVariant { .. }
        | MirInstruction::MakeResult { .. }
        | MirInstruction::UnwrapResult { .. }
        | MirInstruction::MakeOption { .. }
        | MirInstruction::UnwrapOption { .. }
        | MirInstruction::ListGet { .. }
        | MirInstruction::MapGet { .. }
        | MirInstruction::StringBuilderFinish { .. }
        | MirInstruction::GetField { .. }
        | MirInstruction::ListLen { .. }
        | MirInstruction::Binary { .. }
        | MirInstruction::StringConcat { .. }
        | MirInstruction::SetField { .. }
        | MirInstruction::Await { .. }
        | MirInstruction::Select { .. }
        | MirInstruction::Cancel { .. }
        | MirInstruction::Join { .. }
        | MirInstruction::Discard { .. } => Ok(()),
        MirInstruction::TryResult { cleanup, .. } => {
            for place in cleanup {
                check_live(*place, moved_places)?;
            }
            Ok(())
        }
    }
}

struct InstructionVerification<'a> {
    defined: &'a mut BTreeSet<ValueId>,
    used: &'a mut Vec<ValueId>,
    moved_places: &'a mut BTreeSet<PlaceId>,
    type_count: usize,
    functions: &'a [MirFunction],
    external_imports: &'a [MirExternalImport],
}

pub(super) fn verify_instruction(
    function: &MirFunction,
    instruction: &MirInstruction,
    verification: InstructionVerification<'_>,
) -> Result<(), MirValidationError> {
    let InstructionVerification {
        defined,
        used,
        moved_places,
        type_count,
        functions,
        external_imports,
    } = verification;
    let define = |value: ValueId, defined: &mut BTreeSet<ValueId>| {
        if value.index() >= function.value_count as usize || !defined.insert(value) {
            Err(MirValidationError::InvalidValueDefinition {
                function: function.id,
                value,
            })
        } else {
            Ok(())
        }
    };
    let check_place = |place: PlaceId| {
        if place.index() >= function.place_count as usize {
            Err(MirValidationError::InvalidPlace {
                function: function.id,
                place,
            })
        } else {
            Ok(())
        }
    };
    let check_live_place = |place: PlaceId, moved_places: &BTreeSet<PlaceId>| {
        check_place(place)?;
        if moved_places.contains(&place) {
            Err(MirValidationError::UseAfterMove {
                function: function.id,
                place,
            })
        } else {
            Ok(())
        }
    };
    match instruction {
        MirInstruction::LoadLiteral { destination, .. }
        | MirInstruction::MakeList { destination, .. }
        | MirInstruction::MakeMap { destination, .. }
        | MirInstruction::MakeObject { destination, .. }
        | MirInstruction::MakeResult { destination, .. }
        | MirInstruction::ListGet { destination, .. }
        | MirInstruction::MapGet { destination, .. }
        | MirInstruction::GetField { destination, .. }
        | MirInstruction::ListLen { destination, .. } => define(*destination, defined),
        MirInstruction::MakeStruct { destination, .. }
        | MirInstruction::MakeVariant { destination, .. } => define(*destination, defined),
        MirInstruction::UnwrapResult {
            destination,
            source,
            ..
        } => {
            define(*destination, defined)?;
            used.push(*source);
            Ok(())
        }
        MirInstruction::MakeOption { destination, .. } => define(*destination, defined),
        MirInstruction::UnwrapOption {
            destination,
            source,
        } => {
            define(*destination, defined)?;
            used.push(*source);
            Ok(())
        }
        MirInstruction::ListAppend {
            destination,
            list,
            values,
        } => {
            check_live_place(*list, moved_places)?;
            define(*destination, defined)?;
            used.push(*values);
            Ok(())
        }
        MirInstruction::ListClear { destination, list }
        | MirInstruction::ListPop { destination, list } => {
            check_live_place(*list, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::ListPush {
            destination,
            list,
            value,
        } => {
            check_live_place(*list, moved_places)?;
            define(*destination, defined)?;
            used.push(*value);
            Ok(())
        }
        MirInstruction::ListRemoveAt {
            destination,
            list,
            index,
        } => {
            check_live_place(*list, moved_places)?;
            define(*destination, defined)?;
            used.push(*index);
            Ok(())
        }
        MirInstruction::ListSet {
            destination,
            list,
            index,
            value,
        } => {
            check_live_place(*list, moved_places)?;
            define(*destination, defined)?;
            used.push(*index);
            used.push(*value);
            Ok(())
        }
        MirInstruction::SetClear { destination, set } => {
            check_live_place(*set, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::SetInsert {
            destination,
            set,
            value,
        }
        | MirInstruction::SetRemove {
            destination,
            set,
            value,
        } => {
            check_live_place(*set, moved_places)?;
            define(*destination, defined)?;
            used.push(*value);
            Ok(())
        }
        MirInstruction::DequeClear { destination, deque }
        | MirInstruction::DequePopBack { destination, deque }
        | MirInstruction::DequePopFront { destination, deque } => {
            check_live_place(*deque, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::DequePushBack {
            destination,
            deque,
            value,
        }
        | MirInstruction::DequePushFront {
            destination,
            deque,
            value,
        } => {
            check_live_place(*deque, moved_places)?;
            define(*destination, defined)?;
            used.push(*value);
            Ok(())
        }
        MirInstruction::SortedMapClear { destination, map } => {
            check_live_place(*map, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::SortedMapInsert {
            destination,
            map,
            key,
            value,
        } => {
            check_live_place(*map, moved_places)?;
            define(*destination, defined)?;
            used.push(*key);
            used.push(*value);
            Ok(())
        }
        MirInstruction::SortedMapRemove {
            destination,
            map,
            key,
        } => {
            check_live_place(*map, moved_places)?;
            define(*destination, defined)?;
            used.push(*key);
            Ok(())
        }
        MirInstruction::SortedSetClear { destination, set } => {
            check_live_place(*set, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::SortedSetInsert {
            destination,
            set,
            value,
        }
        | MirInstruction::SortedSetRemove {
            destination,
            set,
            value,
        } => {
            check_live_place(*set, moved_places)?;
            define(*destination, defined)?;
            used.push(*value);
            Ok(())
        }
        MirInstruction::BufferClear {
            destination,
            buffer,
        } => {
            check_live_place(*buffer, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::StringBuilderPush {
            destination,
            builder,
            value,
        } => {
            check_live_place(*builder, moved_places)?;
            define(*destination, defined)?;
            used.push(*value);
            Ok(())
        }
        MirInstruction::StringBuilderFinish {
            destination,
            builder,
        } => {
            define(*destination, defined)?;
            used.push(*builder);
            Ok(())
        }
        MirInstruction::MapClear { destination, map } => {
            check_live_place(*map, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::MapInsert {
            destination,
            map,
            key,
            value,
        }
        | MirInstruction::MapInsertOld {
            destination,
            map,
            key,
            value,
        } => {
            check_live_place(*map, moved_places)?;
            define(*destination, defined)?;
            used.push(*key);
            used.push(*value);
            Ok(())
        }
        MirInstruction::MapRemove {
            destination,
            map,
            key,
        } => {
            check_live_place(*map, moved_places)?;
            define(*destination, defined)?;
            used.push(*key);
            Ok(())
        }
        MirInstruction::ReadPlace { destination, place } => {
            check_live_place(*place, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::BorrowRead { destination, place } => {
            check_live_place(*place, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::TakePlace { destination, place } => {
            check_live_place(*place, moved_places)?;
            moved_places.insert(*place);
            define(*destination, defined)
        }
        MirInstruction::Manage {
            destination,
            source,
        } => {
            define(*destination, defined)?;
            used.push(*source);
            Ok(())
        }
        MirInstruction::Retain { place } => check_live_place(*place, moved_places),
        MirInstruction::Drop { place } => {
            check_live_place(*place, moved_places)?;
            moved_places.insert(*place);
            Ok(())
        }
        MirInstruction::AcquireResource { place, source, .. } => {
            check_place(*place)?;
            moved_places.remove(place);
            used.push(*source);
            Ok(())
        }
        MirInstruction::ReleaseResource { place } => {
            check_live_place(*place, moved_places)?;
            moved_places.insert(*place);
            Ok(())
        }
        MirInstruction::WritePlace { place, value } => {
            check_place(*place)?;
            moved_places.remove(place);
            used.push(*value);
            Ok(())
        }
        MirInstruction::SetField { base, value, .. } => {
            used.push(*base);
            used.push(*value);
            Ok(())
        }
        MirInstruction::Binary {
            destination,
            left,
            right,
            ..
        }
        | MirInstruction::StringConcat {
            destination,
            left,
            right,
        } => {
            define(*destination, defined)?;
            used.push(*left);
            used.push(*right);
            Ok(())
        }
        MirInstruction::TryResult {
            destination,
            source,
            cleanup,
        } => {
            define(*destination, defined)?;
            used.push(*source);
            for place in cleanup {
                check_live_place(*place, moved_places)?;
            }
            Ok(())
        }
        MirInstruction::Call {
            destination,
            target,
            arguments,
        } => {
            define(*destination, defined)?;
            if let MirCallTarget::FunctionInstance {
                type_substitutions, ..
            } = target
            {
                for (parameter, argument) in type_substitutions {
                    if parameter.index() >= type_count || argument.index() >= type_count {
                        return Err(MirValidationError::InvalidType {
                            function: function.id,
                            ty: if parameter.index() >= type_count {
                                *parameter
                            } else {
                                *argument
                            },
                        });
                    }
                }
            }
            let expected_modes = match target {
                MirCallTarget::Function(target) if target.index() < functions.len() => functions
                    [target.index()]
                .signature
                .parameter_modes()
                .to_vec(),
                MirCallTarget::FunctionInstance {
                    function: target, ..
                } if target.index() < functions.len() => functions[target.index()]
                    .signature
                    .parameter_modes()
                    .to_vec(),
                MirCallTarget::Function(target) => {
                    return Err(MirValidationError::InvalidFunctionTarget {
                        function: function.id,
                        target: *target,
                    });
                }
                MirCallTarget::FunctionInstance {
                    function: target, ..
                } => {
                    return Err(MirValidationError::InvalidFunctionTarget {
                        function: function.id,
                        target: *target,
                    });
                }
                MirCallTarget::Dynamic {
                    dispatch,
                    parameter_modes,
                } => {
                    if dispatch.is_empty() {
                        return Err(MirValidationError::EmptyDynamicDispatch {
                            function: function.id,
                        });
                    }
                    for (receiver, target) in dispatch.iter() {
                        if receiver.index() >= type_count {
                            return Err(MirValidationError::InvalidDynamicDispatchType {
                                function: function.id,
                                ty: *receiver,
                            });
                        }
                        let Some(callee) = functions.get(target.index()) else {
                            return Err(MirValidationError::InvalidFunctionTarget {
                                function: function.id,
                                target: *target,
                            });
                        };
                        if callee.signature.parameter_modes() != parameter_modes.as_ref() {
                            return Err(MirValidationError::DynamicDispatchSignatureMismatch {
                                function: function.id,
                                target: *target,
                            });
                        }
                    }
                    parameter_modes.to_vec()
                }
                MirCallTarget::Builtin {
                    id,
                    parameter_modes,
                    type_arguments,
                } if builtin_descriptor(*id).is_some() => parameter_modes.to_vec(),
                MirCallTarget::Builtin { id, .. } => {
                    return Err(MirValidationError::InvalidBuiltinTarget {
                        function: function.id,
                        target: *id,
                    });
                }
                MirCallTarget::External(target) if target.index() < external_imports.len() => {
                    external_imports[target.index()]
                        .signature
                        .parameters
                        .iter()
                        .map(|parameter| match parameter.effect {
                            rsscript_abi_model::DataEffect::Read => MirParameterMode::Read,
                            rsscript_abi_model::DataEffect::Mut => MirParameterMode::Mut,
                            rsscript_abi_model::DataEffect::Take => MirParameterMode::Take,
                        })
                        .collect()
                }
                MirCallTarget::External(target) => {
                    return Err(MirValidationError::InvalidExternalTarget {
                        function: function.id,
                        target: *target,
                    });
                }
            };
            if let MirCallTarget::Builtin {
                id, type_arguments, ..
            } = target
            {
                let expected_type_arguments = match builtin_descriptor(*id) {
                    Some(descriptor)
                        if matches!(descriptor.vm_name, "JsonDecode" | "JsonDecodeText") =>
                    {
                        1
                    }
                    Some(_) => 0,
                    None => unreachable!("builtin target was validated above"),
                };
                if type_arguments.len() != expected_type_arguments {
                    return Err(MirValidationError::BuiltinTypeArgumentArity {
                        function: function.id,
                        target: *id,
                        expected: expected_type_arguments,
                        actual: type_arguments.len(),
                    });
                }
                for ty in type_arguments {
                    if ty.index() >= type_count {
                        return Err(MirValidationError::InvalidBuiltinTypeArgument {
                            function: function.id,
                            ty: *ty,
                        });
                    }
                }
            }
            if arguments.len() != expected_modes.len() {
                return Err(MirValidationError::CallArityMismatch {
                    function: function.id,
                    expected: expected_modes.len(),
                    actual: arguments.len(),
                });
            }
            for (parameter, (argument, expected)) in
                arguments.iter().zip(expected_modes).enumerate()
            {
                let actual = argument.mode();
                if !call_argument_compatible(actual, expected) {
                    return Err(MirValidationError::CallArgumentModeMismatch {
                        function: function.id,
                        parameter,
                        expected,
                        actual,
                    });
                }
                match argument {
                    MirCallArgument::Value(value) => used.push(*value),
                    MirCallArgument::BorrowRead(place) | MirCallArgument::BorrowMut(place) => {
                        check_live_place(*place, moved_places)?;
                    }
                    MirCallArgument::Take(place) => {
                        check_live_place(*place, moved_places)?;
                        moved_places.insert(*place);
                    }
                }
            }
            Ok(())
        }
        MirInstruction::MakeClosure {
            destination,
            function: target,
            captures,
        } => {
            define(*destination, defined)?;
            let Some(callee) = functions.get(target.index()) else {
                return Err(MirValidationError::InvalidFunctionTarget {
                    function: function.id,
                    target: *target,
                });
            };
            if captures.len() != callee.captures().len() {
                return Err(MirValidationError::ClosureCaptureArityMismatch {
                    function: function.id,
                    target: *target,
                    expected: callee.captures().len(),
                    actual: captures.len(),
                });
            }
            for (index, (argument, capture)) in captures.iter().zip(callee.captures()).enumerate() {
                let actual = argument.mode();
                if !call_argument_compatible(actual, capture.mode()) {
                    return Err(MirValidationError::ClosureCaptureModeMismatch {
                        function: function.id,
                        target: *target,
                        capture: index,
                        expected: capture.mode(),
                        actual,
                    });
                }
                match argument {
                    MirCallArgument::Value(value) => used.push(*value),
                    MirCallArgument::BorrowRead(place) | MirCallArgument::BorrowMut(place) => {
                        check_live_place(*place, moved_places)?;
                    }
                    MirCallArgument::Take(place) => {
                        check_live_place(*place, moved_places)?;
                        moved_places.insert(*place);
                    }
                }
            }
            Ok(())
        }
        MirInstruction::CallClosure {
            destination,
            closure,
            parameter_types,
            parameter_modes,
            arguments,
        } => {
            define(*destination, defined)?;
            used.push(*closure);
            if parameter_types.len() != parameter_modes.len() {
                return Err(MirValidationError::ClosureParameterModeCount {
                    function: function.id,
                    types: parameter_types.len(),
                    modes: parameter_modes.len(),
                });
            }
            for ty in parameter_types {
                if ty.index() >= type_count {
                    return Err(MirValidationError::InvalidClosureParameterType {
                        function: function.id,
                        ty: *ty,
                    });
                }
            }
            if arguments.len() != parameter_modes.len() {
                return Err(MirValidationError::CallArityMismatch {
                    function: function.id,
                    expected: parameter_modes.len(),
                    actual: arguments.len(),
                });
            }
            for (parameter, (argument, expected)) in arguments
                .iter()
                .zip(parameter_modes.iter().copied())
                .enumerate()
            {
                let actual = argument.mode();
                if !call_argument_compatible(actual, expected) {
                    return Err(MirValidationError::CallArgumentModeMismatch {
                        function: function.id,
                        parameter,
                        expected,
                        actual,
                    });
                }
                match argument {
                    MirCallArgument::Value(value) => used.push(*value),
                    MirCallArgument::BorrowRead(place) | MirCallArgument::BorrowMut(place) => {
                        check_live_place(*place, moved_places)?;
                    }
                    MirCallArgument::Take(place) => {
                        check_live_place(*place, moved_places)?;
                        moved_places.insert(*place);
                    }
                }
            }
            Ok(())
        }
        MirInstruction::Spawn {
            target, arguments, ..
        } => {
            let Some(callee) = functions.get(target.index()) else {
                return Err(MirValidationError::InvalidFunctionTarget {
                    function: function.id,
                    target: *target,
                });
            };
            if !callee.signature.is_async() {
                return Err(MirValidationError::SpawnTargetNotAsync {
                    function: function.id,
                    target: *target,
                });
            }
            if arguments.len() != callee.signature.parameter_modes().len() {
                return Err(MirValidationError::CallArityMismatch {
                    function: function.id,
                    expected: callee.signature.parameter_modes().len(),
                    actual: arguments.len(),
                });
            }
            for (parameter, (argument, expected)) in arguments
                .iter()
                .zip(callee.signature.parameter_modes())
                .enumerate()
            {
                let actual = argument.mode();
                if !call_argument_compatible(actual, *expected) {
                    return Err(MirValidationError::CallArgumentModeMismatch {
                        function: function.id,
                        parameter,
                        expected: *expected,
                        actual,
                    });
                }
                match argument {
                    MirCallArgument::Value(value) => used.push(*value),
                    MirCallArgument::BorrowRead(place) | MirCallArgument::BorrowMut(place) => {
                        check_live_place(*place, moved_places)?;
                    }
                    MirCallArgument::Take(place) => {
                        check_live_place(*place, moved_places)?;
                        moved_places.insert(*place);
                    }
                }
            }
            Ok(())
        }
        MirInstruction::Await { destination, .. } => define(*destination, defined),
        MirInstruction::Select { winner, value, .. } => {
            define(*winner, defined)?;
            define(*value, defined)
        }
        MirInstruction::Cancel { .. } | MirInstruction::Join { .. } => Ok(()),
        MirInstruction::Discard { value } => {
            used.push(*value);
            Ok(())
        }
    }
}

pub(super) fn call_argument_compatible(actual: MirCallArgumentMode, expected: MirParameterMode) -> bool {
    matches!(
        (actual, expected),
        (
            MirCallArgumentMode::Value | MirCallArgumentMode::Read,
            MirParameterMode::Read
        ) | (MirCallArgumentMode::Mut, MirParameterMode::Mut)
            | (MirCallArgumentMode::Take, MirParameterMode::Take)
    )
}

pub(super) fn verify_terminator(
    function: &MirFunction,
    terminator: &MirTerminator,
    used: &mut Vec<ValueId>,
) -> Result<(), MirValidationError> {
    let check_target = |target: BlockId| {
        if target.index() >= function.blocks.len() {
            Err(MirValidationError::InvalidBlockTarget {
                function: function.id,
                target,
            })
        } else {
            Ok(())
        }
    };
    match terminator {
        MirTerminator::Return(value) => {
            if let Some(value) = value {
                used.push(*value);
            }
        }
        MirTerminator::Jump(target) => check_target(*target)?,
        MirTerminator::Branch {
            condition,
            then_target,
            else_target,
        } => {
            used.push(*condition);
            check_target(*then_target)?;
            check_target(*else_target)?;
        }
        MirTerminator::MatchVariant {
            expected,
            match_target,
            else_target,
            ..
        } => {
            if expected.is_empty() {
                return Err(MirValidationError::InvalidVariantTag {
                    function: function.id,
                });
            }
            check_target(*match_target)?;
            check_target(*else_target)?;
        }
        MirTerminator::MatchResult {
            ok_target,
            err_target,
            ..
        } => {
            check_target(*ok_target)?;
            check_target(*err_target)?;
        }
        MirTerminator::MatchOption {
            some_target,
            none_target,
            ..
        } => {
            check_target(*some_target)?;
            check_target(*none_target)?;
        }
        MirTerminator::Unreachable => {}
    }
    Ok(())
}
