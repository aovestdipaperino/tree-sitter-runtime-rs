//! Part of the `query.c` port (see `query.rs`): query analysis, C lines 1308-2178.
//!
//! `query_perform_analysis` walks the parse table for one pattern, and `query_analyze_patterns`
//! uses it to compute which steps can fail, which patterns can never match and which repetition
//! symbols can start non-rooted patterns. `ts_query__dump_steps` (debug-only) is not ported.
#![allow(clippy::missing_safety_doc, non_upper_case_globals)]
use crate::api::*;
use crate::array::Array;
use crate::language::*;
use crate::parser_abi::*;
use crate::query::TSQuery;
use crate::query::*;
use crate::ts_assert;
use core::ptr;

fn empty_entry() -> AnalysisStateEntry {
    AnalysisStateEntry {
        parse_state: 0,
        parent_symbol: 0,
        child_index: 0,
        field_id: 0,
        done: false,
    }
}

/// `(AnalysisState) { .step_index, .stack = {[0] = {...}}, .depth = 1, .root_symbol }`.
fn initial_state(
    step_index: u16,
    parse_state: TSStateId,
    parent_symbol: TSSymbol,
    root_symbol: TSSymbol,
) -> AnalysisState {
    let mut stack = [empty_entry(); MAX_ANALYSIS_STATE_DEPTH];
    stack[0] = AnalysisStateEntry {
        parse_state,
        parent_symbol,
        child_index: 0,
        field_id: 0,
        done: false,
    };
    AnalysisState {
        stack,
        depth: 1,
        step_index,
        root_symbol,
    }
}

/// `array_insert_sorted_by(array, field, value)` for integer-keyed elements.
unsafe fn insert_sorted_by<T: Copy>(array: *mut Array<T>, key: impl Fn(&T) -> u32, value: T) {
    unsafe {
        let (index, exists) = (*array).search_sorted_by(&key, key(&value));
        if !exists {
            (*array).insert(index, value);
        }
    }
}

/// Walk the subgraph for this non-terminal, tracking all of the possible sequences of progress
/// within the pattern.
pub unsafe fn query_perform_analysis(
    this: *mut TSQuery,
    subgraphs: *const AnalysisSubgraphArray,
    analysis: *mut QueryAnalysis,
) {
    unsafe {
        let language = (*this).language;
        let mut recursion_depth_limit = 0u32;
        let mut prev_final_step_count = 0u32;
        (*analysis).final_step_indices.clear();
        (*analysis).finished_parent_symbols.clear();

        let mut iteration = 0u32;
        loop {
            if iteration == MAX_ANALYSIS_ITERATION_COUNT {
                (*analysis).did_abort = true;
                break;
            }
            iteration = iteration.wrapping_add(1);

            // If no further progress can be made within the current recursion depth limit, then
            // bump the depth limit by one, and continue to process the states the exceeded the
            // limit. But only allow this if progress has been made since the last time the depth
            // limit was increased.
            if (*analysis).states.size == 0 {
                if (*analysis).deeper_states.size > 0
                    && (*analysis).final_step_indices.size > prev_final_step_count
                {
                    prev_final_step_count = (*analysis).final_step_indices.size;
                    recursion_depth_limit += 1;
                    core::mem::swap(&mut (*analysis).states, &mut (*analysis).deeper_states);
                    continue;
                }

                break;
            }

            analysis_state_set_clear(&mut (*analysis).next_states, &mut (*analysis).state_pool);
            let state_count = (*analysis).states.size;
            'states: for j in 0..state_count {
                let state: *mut AnalysisState = *(*analysis).states.get(j);

                // For efficiency, it's important to avoid processing the same analysis state more
                // than once. To achieve this, keep the states in order of ascending position within
                // their hypothetical syntax trees. In each iteration of this loop, start by advancing
                // the states that have made the least progress. Avoid advancing states that have already
                // made more progress.
                if (*analysis).next_states.size > 0 {
                    let comparison = analysis_state_compare(
                        &state,
                        (*analysis).next_states.back() as *const *mut AnalysisState,
                    );
                    if comparison == 0 {
                        analysis_state_set_insert_sorted(
                            &mut (*analysis).next_states,
                            &mut (*analysis).state_pool,
                            state,
                        );
                        continue 'states;
                    } else if comparison > 0 {
                        for jj in j..state_count {
                            analysis_state_set_push(
                                &mut (*analysis).next_states,
                                &mut (*analysis).state_pool,
                                *(*analysis).states.get(jj),
                            );
                        }
                        break 'states;
                    }
                }

                let parse_state = (*analysis_state_top(state)).parse_state;
                let parent_symbol = (*analysis_state_top(state)).parent_symbol;
                let parent_field_id: TSFieldId = (*analysis_state_top(state)).field_id;
                let child_index = u32::from((*analysis_state_top(state)).child_index);
                let step: *const QueryStep = (*this).steps.get(u32::from((*state).step_index));

                let (subgraph_index, exists) = (*subgraphs)
                    .search_sorted_by(|s| u32::from(s.symbol), u32::from(parent_symbol));
                if !exists {
                    continue 'states;
                }
                let subgraph = (*subgraphs).get(subgraph_index);

                // Follow every possible path in the parse table, but only visit states that
                // are part of the subgraph for the current symbol.
                let mut lookahead_iterator = ts_language_lookaheads(language, parse_state);
                while ts_lookahead_iterator_next_inner(&mut lookahead_iterator) {
                    let sym = lookahead_iterator.symbol;

                    let mut successor = AnalysisSubgraphNode {
                        state: parse_state,
                        production_id: 0,
                        child_index: (child_index as u8) & ANALYSIS_SUBGRAPH_CHILD_INDEX_MASK,
                        done: false,
                    };
                    if lookahead_iterator.action_count != 0 {
                        let action = lookahead_iterator
                            .actions
                            .add(usize::from(lookahead_iterator.action_count) - 1);
                        if (*action).kind == TS_PARSE_ACTION_TYPE_SHIFT {
                            if !(*action).shift.extra {
                                successor.state = (*action).shift.state;
                                successor.child_index = successor.child_index.wrapping_add(1)
                                    & ANALYSIS_SUBGRAPH_CHILD_INDEX_MASK;
                            }
                        } else {
                            continue;
                        }
                    } else if lookahead_iterator.next_state != 0 {
                        successor.state = lookahead_iterator.next_state;
                        successor.child_index = successor.child_index.wrapping_add(1)
                            & ANALYSIS_SUBGRAPH_CHILD_INDEX_MASK;
                    } else {
                        continue;
                    }

                    let (mut node_index, _) = (*subgraph).nodes.search_sorted_with(
                        |a, n: AnalysisSubgraphNode| analysis_subgraph_node_compare(a, &n),
                        successor,
                    );
                    while node_index < (*subgraph).nodes.size {
                        let node = (*subgraph).nodes.get(node_index);
                        node_index += 1;
                        if (*node).state != successor.state
                            || (*node).child_index != successor.child_index
                        {
                            break;
                        }

                        // Use the subgraph to determine what alias and field will eventually be applied
                        // to this child node.
                        let alias = ts_language_alias_at(
                            language,
                            u32::from((*node).production_id),
                            child_index,
                        );
                        let visible_symbol: TSSymbol = if alias != 0 {
                            alias
                        } else if (*(*language).symbol_metadata.add(usize::from(sym))).visible {
                            *(*language).public_symbol_map.add(usize::from(sym))
                        } else {
                            0
                        };
                        let mut field_id: TSFieldId = parent_field_id;
                        if field_id == 0 {
                            let mut field_map: *const TSFieldMapEntry = ptr::null();
                            let mut field_map_end: *const TSFieldMapEntry = ptr::null();
                            ts_language_field_map(
                                language,
                                u32::from((*node).production_id),
                                &mut field_map,
                                &mut field_map_end,
                            );
                            while field_map != field_map_end {
                                if !(*field_map).inherited
                                    && u32::from((*field_map).child_index) == child_index
                                {
                                    field_id = (*field_map).field_id;
                                    break;
                                }
                                field_map = field_map.add(1);
                            }
                        }

                        // Create a new state that has advanced past this hypothetical subtree.
                        let mut next_state: AnalysisState = *state;
                        let next_state_ptr: *mut AnalysisState = &mut next_state;
                        let mut next_state_top = analysis_state_top(next_state_ptr);
                        (*next_state_top).child_index = u16::from(successor.child_index);
                        (*next_state_top).parse_state = successor.state;
                        if (*node).done {
                            (*next_state_top).done = true;
                        }

                        // Determine if this hypothetical child node would match the current step
                        // of the query pattern.
                        let mut does_match = false;

                        // ERROR nodes can appear anywhere, so if the step is
                        // looking for an ERROR node, consider it potentially matchable.
                        if (*step).symbol == ts_builtin_sym_error {
                            does_match = true;
                        } else if visible_symbol != 0 {
                            does_match = true;
                            if (*step).symbol == WILDCARD_SYMBOL {
                                if (*step).is_named
                                    && !(*(*language)
                                        .symbol_metadata
                                        .add(usize::from(visible_symbol)))
                                    .named
                                {
                                    does_match = false;
                                }
                            } else if (*step).symbol != visible_symbol {
                                does_match = false;
                            }
                            if (*step).field != 0 && (*step).field != field_id {
                                does_match = false;
                            }
                            if (*step).supertype_symbol != 0
                                && !analysis_state_has_supertype(state, (*step).supertype_symbol)
                            {
                                does_match = false;
                            }
                        }
                        // If this child is hidden, then descend into it and walk through its children.
                        // If the top entry of the stack is at the end of its rule, then that entry can
                        // be replaced. Otherwise, push a new entry onto the stack.
                        else if u32::from(sym) >= (*language).token_count {
                            if !(*next_state_top).done {
                                if usize::from(next_state.depth) + 1 >= MAX_ANALYSIS_STATE_DEPTH {
                                    (*analysis).did_abort = true;
                                    continue;
                                }

                                next_state.depth += 1;
                                next_state_top = analysis_state_top(next_state_ptr);
                            }

                            *next_state_top = AnalysisStateEntry {
                                parse_state,
                                parent_symbol: sym,
                                child_index: 0,
                                field_id: field_id & ANALYSIS_FIELD_ID_MASK,
                                done: false,
                            };

                            if analysis_state_recursion_depth(next_state_ptr)
                                > recursion_depth_limit
                            {
                                analysis_state_set_insert_sorted(
                                    &mut (*analysis).deeper_states,
                                    &mut (*analysis).state_pool,
                                    next_state_ptr,
                                );
                                continue;
                            }
                        }

                        // Pop from the stack when this state reached the end of its current syntax node.
                        while next_state.depth > 0 && (*next_state_top).done {
                            next_state.depth -= 1;
                            next_state_top = analysis_state_top(next_state_ptr);
                        }

                        // If this hypothetical child did match the current step of the query pattern,
                        // then advance to the next step at the current depth. This involves skipping
                        // over any descendant steps of the current child.
                        let mut next_step: *const QueryStep = step;
                        if does_match {
                            loop {
                                next_state.step_index = next_state.step_index.wrapping_add(1);
                                next_step = (*this).steps.get(u32::from(next_state.step_index));
                                if (*next_step).depth == PATTERN_DONE_MARKER
                                    || (*next_step).depth <= (*step).depth
                                {
                                    break;
                                }
                            }
                        } else if successor.state == parse_state {
                            continue;
                        }

                        loop {
                            // Skip pass-through states. Although these states have alternatives, they are only
                            // used to implement repetitions, and query analysis does not need to process
                            // repetitions in order to determine whether steps are possible and definite.
                            if (*next_step).is_pass_through {
                                next_state.step_index = next_state.step_index.wrapping_add(1);
                                next_step = next_step.add(1);
                                continue;
                            }

                            // If the pattern is finished or hypothetical parent node is complete, then
                            // record that matching can terminate at this step of the pattern. Otherwise,
                            // add this state to the list of states to process on the next iteration.
                            if !(*next_step).is_dead_end {
                                let did_finish_pattern =
                                    (*(*this).steps.get(u32::from(next_state.step_index))).depth
                                        != (*step).depth;
                                if did_finish_pattern {
                                    insert_sorted_by(
                                        &mut (*analysis).finished_parent_symbols,
                                        |v| u32::from(*v),
                                        (*state).root_symbol,
                                    );
                                } else if next_state.depth == 0 {
                                    insert_sorted_by(
                                        &mut (*analysis).final_step_indices,
                                        |v| u32::from(*v),
                                        next_state.step_index,
                                    );
                                } else {
                                    analysis_state_set_insert_sorted(
                                        &mut (*analysis).next_states,
                                        &mut (*analysis).state_pool,
                                        next_state_ptr,
                                    );
                                }
                            }

                            // If the state has advanced to a step with an alternative step, then add another state
                            // at that alternative step. This process is simpler than the process of actually matching a
                            // pattern during query execution, because for the purposes of query analysis, there is no
                            // need to process repetitions.
                            if does_match
                                && (*next_step).alternative_index != NONE
                                && (*next_step).alternative_index > next_state.step_index
                            {
                                next_state.step_index = (*next_step).alternative_index;
                                next_step = (*this).steps.get(u32::from(next_state.step_index));
                            } else {
                                break;
                            }
                        }
                    }
                }
            }

            core::mem::swap(&mut (*analysis).states, &mut (*analysis).next_states);
        }
    }
}

pub unsafe fn query_analyze_patterns(this: *mut TSQuery, error_offset: *mut u32) -> bool {
    unsafe {
        let language = (*this).language;
        let mut non_rooted_pattern_start_steps: Array<u16> = Array::new();
        for i in 0..(*this).pattern_map.size {
            let pattern = (*this).pattern_map.get(i);
            if !(*pattern).is_rooted {
                let step = (*this).steps.get(u32::from((*pattern).step_index));
                if (*step).symbol != WILDCARD_SYMBOL {
                    non_rooted_pattern_start_steps.push(i as u16);
                }
            }
        }

        // Walk forward through all of the steps in the query, computing some
        // basic information about each step. Mark all of the steps that contain
        // captures, and record the indices of all of the steps that have child steps.
        let mut parent_step_indices: Array<u32> = Array::new();
        let mut all_patterns_are_valid = true;

        'supertype_cleanup: {
            for i in 0..(*this).steps.size {
                let step = (*this).steps.get(i);
                if (*step).depth == PATTERN_DONE_MARKER {
                    (*step).parent_pattern_guaranteed = true;
                    (*step).root_pattern_guaranteed = true;
                    continue;
                }

                let mut has_children = false;
                let is_wildcard = (*step).symbol == WILDCARD_SYMBOL;
                (*step).contains_captures = (*step).capture_ids[0] != NONE;
                for j in (i + 1)..(*this).steps.size {
                    let next_step = (*this).steps.get(j);
                    if (*next_step).depth == PATTERN_DONE_MARKER
                        || (*next_step).depth <= (*step).depth
                    {
                        break;
                    }
                    if (*next_step).capture_ids[0] != NONE {
                        (*step).contains_captures = true;
                    }
                    if !is_wildcard {
                        (*next_step).root_pattern_guaranteed = true;
                        (*next_step).parent_pattern_guaranteed = true;
                    }
                    has_children = true;
                }

                if has_children {
                    if !is_wildcard {
                        parent_step_indices.push(i);
                    } else if (*step).supertype_symbol != 0
                        && (*language).abi_version >= LANGUAGE_VERSION_WITH_RESERVED_WORDS
                    {
                        // Look at the child steps to see if any aren't valid subtypes for this supertype.
                        let mut subtype_length = 0u32;
                        let subtypes = ts_language_subtypes(
                            language,
                            (*step).supertype_symbol,
                            &mut subtype_length,
                        );

                        for j in (i + 1)..(*this).steps.size {
                            let child_step = (*this).steps.get(j);
                            if (*child_step).depth == PATTERN_DONE_MARKER
                                || (*child_step).depth <= (*step).depth
                            {
                                break;
                            }
                            if u32::from((*child_step).depth) == u32::from((*step).depth) + 1
                                && (*child_step).symbol != WILDCARD_SYMBOL
                            {
                                let mut is_valid_subtype = false;
                                for k in 0..subtype_length {
                                    if (*child_step).symbol == *subtypes.add(k as usize) {
                                        is_valid_subtype = true;
                                        break;
                                    }
                                }

                                if !is_valid_subtype {
                                    for offset_idx in 0..(*this).step_offsets.size {
                                        let step_offset = (*this).step_offsets.get(offset_idx);
                                        if u32::from((*step_offset).step_index) >= j {
                                            *error_offset = (*step_offset).byte_offset;
                                            all_patterns_are_valid = false;
                                            break 'supertype_cleanup;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // For every parent symbol in the query, initialize an 'analysis subgraph'.
            // This subgraph lists all of the states in the parse table that are directly
            // involved in building subtrees for this symbol.
            //
            // In addition to the parent symbols in the query, construct subgraphs for all
            // of the hidden symbols in the grammar, because these might occur within
            // one of the parent nodes, such that their children appear to belong to the
            // parent.
            let mut subgraphs: AnalysisSubgraphArray = Array::new();
            for i in 0..parent_step_indices.size {
                let parent_step_index = *parent_step_indices.get(i);
                let parent_symbol = (*(*this).steps.get(parent_step_index)).symbol;
                let subgraph = AnalysisSubgraph {
                    symbol: parent_symbol,
                    start_states: Array::new(),
                    nodes: Array::new(),
                };
                insert_sorted_by(&mut subgraphs, |s| u32::from(s.symbol), subgraph);
            }
            let mut sym = (*language).token_count as u16;
            while sym < (*language).symbol_count as u16 {
                if !ts_language_symbol_metadata(language, sym).visible {
                    let subgraph = AnalysisSubgraph {
                        symbol: sym,
                        start_states: Array::new(),
                        nodes: Array::new(),
                    };
                    insert_sorted_by(&mut subgraphs, |s| u32::from(s.symbol), subgraph);
                }
                sym = sym.wrapping_add(1);
            }

            // Scan the parse table to find the data needed to populate these subgraphs.
            // Collect three things during this scan:
            //   1) All of the parse states where one of these symbols can start.
            //   2) All of the parse states where one of these symbols can end, along
            //      with information about the node that would be created.
            //   3) A list of predecessor states for each state.
            let mut predecessor_map = state_predecessor_map_new(language);
            let mut state: TSStateId = 1;
            while state < (*language).state_count as u16 {
                let mut lookahead_iterator = ts_language_lookaheads(language, state);
                while ts_lookahead_iterator_next_inner(&mut lookahead_iterator) {
                    if lookahead_iterator.action_count != 0 {
                        for i in 0..usize::from(lookahead_iterator.action_count) {
                            let action = lookahead_iterator.actions.add(i);
                            if (*action).kind == TS_PARSE_ACTION_TYPE_REDUCE {
                                let mut aliases: *const TSSymbol = ptr::null();
                                let mut aliases_end: *const TSSymbol = ptr::null();
                                ts_language_aliases_for_symbol(
                                    language,
                                    (*action).reduce.symbol,
                                    &mut aliases,
                                    &mut aliases_end,
                                );
                                let mut symbol = aliases;
                                while symbol < aliases_end {
                                    let (subgraph_index, exists) = subgraphs.search_sorted_by(
                                        |s| u32::from(s.symbol),
                                        u32::from(*symbol),
                                    );
                                    if exists {
                                        let subgraph = subgraphs.get(subgraph_index);
                                        if (*subgraph).nodes.size == 0
                                            || (*(*subgraph).nodes.back()).state != state
                                        {
                                            (*subgraph).nodes.push(AnalysisSubgraphNode {
                                                state,
                                                production_id: (*action).reduce.production_id,
                                                child_index: (*action).reduce.child_count
                                                    & ANALYSIS_SUBGRAPH_CHILD_INDEX_MASK,
                                                done: true,
                                            });
                                        }
                                    }
                                    symbol = symbol.add(1);
                                }
                            } else if (*action).kind == TS_PARSE_ACTION_TYPE_SHIFT
                                && !(*action).shift.extra
                            {
                                let next_state = (*action).shift.state;
                                state_predecessor_map_add(&mut predecessor_map, next_state, state);
                            }
                        }
                    } else if lookahead_iterator.next_state != 0 {
                        if lookahead_iterator.next_state != state {
                            state_predecessor_map_add(
                                &mut predecessor_map,
                                lookahead_iterator.next_state,
                                state,
                            );
                        }
                        if ts_language_state_is_primary(language, state) {
                            let mut aliases: *const TSSymbol = ptr::null();
                            let mut aliases_end: *const TSSymbol = ptr::null();
                            ts_language_aliases_for_symbol(
                                language,
                                lookahead_iterator.symbol,
                                &mut aliases,
                                &mut aliases_end,
                            );
                            let mut symbol = aliases;
                            while symbol < aliases_end {
                                let (subgraph_index, exists) = subgraphs
                                    .search_sorted_by(|s| u32::from(s.symbol), u32::from(*symbol));
                                if exists {
                                    let subgraph = subgraphs.get(subgraph_index);
                                    if (*subgraph).start_states.size == 0
                                        || *(*subgraph).start_states.back() != state
                                    {
                                        (*subgraph).start_states.push(state);
                                    }
                                }
                                symbol = symbol.add(1);
                            }
                        }
                    }
                }
                state = state.wrapping_add(1);
            }

            // For each subgraph, compute the preceding states by walking backward
            // from the end states using the predecessor map.
            let mut next_nodes: Array<AnalysisSubgraphNode> = Array::new();
            let mut i = 0u32;
            while i < subgraphs.size {
                let subgraph = subgraphs.get(i);
                if (*subgraph).nodes.size == 0 {
                    (*subgraph).start_states.delete();
                    subgraphs.erase(i);
                    continue;
                }
                next_nodes.assign(&(*subgraph).nodes);
                while next_nodes.size > 0 {
                    let node = next_nodes.pop();
                    if node.child_index > 1 {
                        let mut predecessor_count = 0u32;
                        let predecessors = state_predecessor_map_get(
                            &predecessor_map,
                            node.state,
                            &mut predecessor_count,
                        );
                        for j in 0..predecessor_count {
                            let predecessor_node = AnalysisSubgraphNode {
                                state: *predecessors.add(j as usize),
                                child_index: (node.child_index - 1)
                                    & ANALYSIS_SUBGRAPH_CHILD_INDEX_MASK,
                                production_id: node.production_id,
                                done: false,
                            };
                            let (index, exists) = (*subgraph).nodes.search_sorted_with(
                                |a, n: AnalysisSubgraphNode| analysis_subgraph_node_compare(a, &n),
                                predecessor_node,
                            );
                            if !exists {
                                (*subgraph).nodes.insert(index, predecessor_node);
                                next_nodes.push(predecessor_node);
                            }
                        }
                    }
                }
                i += 1;
            }

            // For each non-terminal pattern, determine if the pattern can successfully match,
            // and identify all of the possible children within the pattern where matching could fail.
            let mut analysis = query_analysis_new();
            for i in 0..parent_step_indices.size {
                let parent_step_index = *parent_step_indices.get(i) as u16;
                let parent_depth = (*(*this).steps.get(u32::from(parent_step_index))).depth;
                let parent_symbol = (*(*this).steps.get(u32::from(parent_step_index))).symbol;
                if parent_symbol == ts_builtin_sym_error {
                    continue;
                }

                // Find the subgraph that corresponds to this pattern's root symbol. If the pattern's
                // root symbol is a terminal, then return an error.
                let (subgraph_index, exists) =
                    subgraphs.search_sorted_by(|s| u32::from(s.symbol), u32::from(parent_symbol));
                if !exists {
                    let first_child_step_index = u32::from(parent_step_index) + 1;
                    let (j, child_exists) = (*this)
                        .step_offsets
                        .search_sorted_by(|o| u32::from(o.step_index), first_child_step_index);
                    ts_assert!(child_exists);
                    *error_offset = (*(*this).step_offsets.get(j)).byte_offset;
                    all_patterns_are_valid = false;
                    break;
                }

                // Initialize an analysis state at every parse state in the table where
                // this parent symbol can occur.
                let subgraph = subgraphs.get(subgraph_index);
                analysis_state_set_clear(&mut analysis.states, &mut analysis.state_pool);
                analysis_state_set_clear(&mut analysis.deeper_states, &mut analysis.state_pool);
                for j in 0..(*subgraph).start_states.size {
                    let parse_state = *(*subgraph).start_states.get(j);
                    let mut initial = initial_state(
                        parent_step_index.wrapping_add(1),
                        parse_state,
                        parent_symbol,
                        parent_symbol,
                    );
                    analysis_state_set_push(
                        &mut analysis.states,
                        &mut analysis.state_pool,
                        &mut initial,
                    );
                }

                analysis.did_abort = false;
                query_perform_analysis(this, &subgraphs, &mut analysis);

                // If this pattern could not be fully analyzed, then every step should
                // be considered fallible.
                if analysis.did_abort {
                    for j in (u32::from(parent_step_index) + 1)..(*this).steps.size {
                        let step = (*this).steps.get(j);
                        if (*step).depth <= parent_depth || (*step).depth == PATTERN_DONE_MARKER {
                            break;
                        }
                        if !(*step).is_dead_end {
                            (*step).parent_pattern_guaranteed = false;
                            (*step).root_pattern_guaranteed = false;
                        }
                    }
                    continue;
                }

                // If this pattern cannot match, store the pattern index so that it can be
                // returned to the caller.
                if analysis.finished_parent_symbols.size == 0 {
                    let impossible_step_index: u16 = if analysis.final_step_indices.size > 0 {
                        *analysis.final_step_indices.back()
                    } else {
                        // If there isn't a final step, then that means the parent step itself is unreachable.
                        parent_step_index
                    };
                    let (mut j, _) = (*this).step_offsets.search_sorted_by(
                        |o| u32::from(o.step_index),
                        u32::from(impossible_step_index),
                    );
                    if j >= (*this).step_offsets.size {
                        j = (*this).step_offsets.size.wrapping_sub(1);
                    }
                    *error_offset = (*(*this).step_offsets.get(j)).byte_offset;
                    all_patterns_are_valid = false;
                    break;
                }

                // Mark as fallible any step where a match terminated.
                // Later, this property will be propagated to all of the step's predecessors.
                for j in 0..analysis.final_step_indices.size {
                    let final_step_index = u32::from(*analysis.final_step_indices.get(j));
                    let step = (*this).steps.get(final_step_index);
                    if (*step).depth != PATTERN_DONE_MARKER
                        && (*step).depth > parent_depth
                        && !(*step).is_dead_end
                    {
                        (*step).parent_pattern_guaranteed = false;
                        (*step).root_pattern_guaranteed = false;
                    }
                }
            }

            // Mark as indefinite any step with captures that are used in predicates.
            let mut predicate_capture_ids: Array<u16> = Array::new();
            for i in 0..(*this).patterns.size {
                let pattern = (*this).patterns.get(i);

                // Gather all of the captures that are used in predicates for this pattern.
                predicate_capture_ids.clear();
                let start = (*pattern).predicate_steps.offset;
                let end = start.wrapping_add((*pattern).predicate_steps.length);
                let mut j = start;
                while j < end {
                    let step = (*this).predicate_steps.get(j);
                    if (*step).type_ == TSQueryPredicateStepTypeCapture {
                        let value_id = (*step).value_id as u16;
                        insert_sorted_by(&mut predicate_capture_ids, |v| u32::from(*v), value_id);
                    }
                    j += 1;
                }

                // Find all of the steps that have these captures.
                let start = (*pattern).steps.offset;
                let end = start.wrapping_add((*pattern).steps.length);
                let mut j = start;
                while j < end {
                    let step = (*this).steps.get(j);
                    for k in 0..MAX_STEP_CAPTURE_COUNT {
                        let capture_id = (*step).capture_ids[k];
                        if capture_id == NONE {
                            break;
                        }
                        let (_, exists) = predicate_capture_ids
                            .search_sorted_by(|v| u32::from(*v), u32::from(capture_id));
                        if exists {
                            (*step).root_pattern_guaranteed = false;
                            break;
                        }
                    }
                    j += 1;
                }
            }

            // Propagate fallibility. If a pattern is fallible at a given step, then it is
            // fallible at all of its preceding steps.
            let mut done = (*this).steps.size == 0;
            while !done {
                done = true;
                let mut i = (*this).steps.size.wrapping_sub(1);
                while i > 0 {
                    let mut step = (*this).steps.get(i);
                    if (*step).depth == PATTERN_DONE_MARKER {
                        i -= 1;
                        continue;
                    }

                    // Determine if this step is definite or has definite alternatives.
                    let mut parent_pattern_guaranteed = false;
                    loop {
                        if (*step).root_pattern_guaranteed {
                            parent_pattern_guaranteed = true;
                            break;
                        }
                        if (*step).alternative_index == NONE
                            || u32::from((*step).alternative_index) < i
                        {
                            break;
                        }
                        step = (*this).steps.get(u32::from((*step).alternative_index));
                    }

                    // If not, mark its predecessor as indefinite.
                    if !parent_pattern_guaranteed {
                        let prev_step = (*this).steps.get(i - 1);
                        if !(*prev_step).is_dead_end
                            && (*prev_step).depth != PATTERN_DONE_MARKER
                            && (*prev_step).root_pattern_guaranteed
                        {
                            (*prev_step).root_pattern_guaranteed = false;
                            done = false;
                        }
                    }
                    i -= 1;
                }
            }

            // Determine which repetition symbols in this language have the possibility
            // of matching non-rooted patterns in this query. These repetition symbols
            // prevent certain optimizations with range restrictions.
            analysis.did_abort = false;
            for i in 0..non_rooted_pattern_start_steps.size {
                let pattern_entry_index = *non_rooted_pattern_start_steps.get(i);
                let pattern_entry = (*this).pattern_map.get(u32::from(pattern_entry_index));

                analysis_state_set_clear(&mut analysis.states, &mut analysis.state_pool);
                analysis_state_set_clear(&mut analysis.deeper_states, &mut analysis.state_pool);
                for j in 0..subgraphs.size {
                    let subgraph = subgraphs.get(j);
                    let metadata = ts_language_symbol_metadata(language, (*subgraph).symbol);
                    if metadata.visible || metadata.named {
                        continue;
                    }

                    for k in 0..(*subgraph).start_states.size {
                        let parse_state = *(*subgraph).start_states.get(k);
                        let mut initial = initial_state(
                            (*pattern_entry).step_index,
                            parse_state,
                            (*subgraph).symbol,
                            (*subgraph).symbol,
                        );
                        analysis_state_set_push(
                            &mut analysis.states,
                            &mut analysis.state_pool,
                            &mut initial,
                        );
                    }
                }

                query_perform_analysis(this, &subgraphs, &mut analysis);

                if analysis.finished_parent_symbols.size > 0 {
                    (*(*this)
                        .patterns
                        .get(u32::from((*pattern_entry).pattern_index)))
                    .is_non_local = true;
                }

                for k in 0..analysis.finished_parent_symbols.size {
                    let symbol = *analysis.finished_parent_symbols.get(k);
                    insert_sorted_by(
                        &mut (*this).repeat_symbols_with_rootless_patterns,
                        |v| u32::from(*v),
                        symbol,
                    );
                }
            }

            // Cleanup
            for i in 0..subgraphs.size {
                (*subgraphs.get(i)).start_states.delete();
                (*subgraphs.get(i)).nodes.delete();
            }
            subgraphs.delete();
            query_analysis_delete(&mut analysis);
            next_nodes.delete();
            predicate_capture_ids.delete();
            state_predecessor_map_delete(&mut predecessor_map);
        }

        // supertype_cleanup:
        non_rooted_pattern_start_steps.delete();
        parent_step_indices.delete();

        all_patterns_are_valid
    }
}
