//! Part of the `query.c` port, see `query.rs`: query construction (C lines 2180-3420).
//!
//! Pattern and predicate parsing, `ts_query_new`/`delete`/`copy` and the small accessors.
#![allow(clippy::missing_safety_doc, non_upper_case_globals)]
use crate::alloc::{ts_free, ts_malloc};
use crate::api::*;
use crate::array::Array;
use crate::language::*;
use crate::query::TSQuery;
use crate::query::*;
use crate::ts_assert;
use core::ffi::{c_char, c_void};
use core::ptr;

unsafe extern "C" {
    fn strncmp(a: *const c_char, b: *const c_char, n: usize) -> core::ffi::c_int;
}

// TEMP STUB: the real `query_analyze_patterns` lives in `query_analysis.rs` (another fork). On
// integration, delete this function and add `use crate::query_analysis::query_analyze_patterns;`.
unsafe fn query_analyze_patterns(_this: *mut TSQuery, _error_offset: *mut u32) -> bool {
    true
}

unsafe fn query_add_negated_fields(
    this: *mut TSQuery,
    step_index: u16,
    field_ids: *const TSFieldId,
    field_count: u16,
) {
    unsafe {
        let step = (*this).steps.get(u32::from(step_index));

        // The negated field array stores a list of field lists, separated by zeros.
        // Try to find the start index of an existing list that matches this new list.
        let mut failed_match = false;
        let mut match_count = 0u32;
        let mut start_i = 0u32;
        for i in 0..(*this).negated_fields.size {
            let existing_field_id = *(*this).negated_fields.get(i);

            // At each zero value, terminate the match attempt. If we've exactly
            // matched the new field list, then reuse this index. Otherwise,
            // start over the matching process.
            if existing_field_id == 0 {
                if match_count == u32::from(field_count) {
                    (*step).negated_field_list_id = start_i as u16;
                    return;
                } else {
                    start_i = i + 1;
                    match_count = 0;
                    failed_match = false;
                }
            }
            // If the existing list matches our new list so far, then advance
            // to the next element of the new list.
            else if match_count < u32::from(field_count)
                && existing_field_id == *field_ids.add(match_count as usize)
                && !failed_match
            {
                match_count += 1;
            }
            // Otherwise, this existing list has failed to match.
            else {
                match_count = 0;
                failed_match = true;
            }
        }

        (*step).negated_field_list_id = (*this).negated_fields.size as u16;
        (*this)
            .negated_fields
            .extend(u32::from(field_count), field_ids);
        (*this).negated_fields.push(0);
    }
}

unsafe fn query_parse_string_literal(this: *mut TSQuery, stream: *mut Stream) -> TSQueryError {
    unsafe {
        let string_start = (*stream).input;
        if (*stream).next != i32::from(b'"') {
            return TSQueryErrorSyntax;
        }
        stream_advance(stream);
        let mut prev_position = (*stream).input;

        let mut is_escaped = false;
        (*this).string_buffer.clear();
        loop {
            if is_escaped {
                is_escaped = false;
                match (*stream).next {
                    n if n == i32::from(b'n') => (*this).string_buffer.push(b'\n' as c_char),
                    n if n == i32::from(b'r') => (*this).string_buffer.push(b'\r' as c_char),
                    n if n == i32::from(b't') => (*this).string_buffer.push(b'\t' as c_char),
                    n if n == i32::from(b'0') => (*this).string_buffer.push(0),
                    _ => (*this)
                        .string_buffer
                        .extend(u32::from((*stream).next_size), (*stream).input),
                }
                prev_position = (*stream).input.add((*stream).next_size as usize);
            } else if (*stream).next == i32::from(b'\\') {
                (*this).string_buffer.extend(
                    (*stream).input.offset_from(prev_position) as u32,
                    prev_position,
                );
                prev_position = (*stream).input.add(1);
                is_escaped = true;
            } else if (*stream).next == i32::from(b'"') {
                (*this).string_buffer.extend(
                    (*stream).input.offset_from(prev_position) as u32,
                    prev_position,
                );
                stream_advance(stream);
                return TSQueryErrorNone;
            } else if (*stream).next == i32::from(b'\n') {
                stream_reset(stream, string_start);
                return TSQueryErrorSyntax;
            }
            if !stream_advance(stream) {
                stream_reset(stream, string_start);
                return TSQueryErrorSyntax;
            }
        }
    }
}

/// Parse a single predicate associated with a pattern, adding it to the query's internal
/// `predicate_steps` array.
unsafe fn query_parse_predicate(this: *mut TSQuery, stream: *mut Stream) -> TSQueryError {
    unsafe {
        if !stream_is_ident_start(stream) {
            return TSQueryErrorSyntax;
        }
        let predicate_name = (*stream).input;
        stream_scan_identifier(stream);
        if (*stream).next != i32::from(b'?') && (*stream).next != i32::from(b'!') {
            return TSQueryErrorSyntax;
        }
        stream_advance(stream);
        let length = (*stream).input.offset_from(predicate_name) as u32;
        let id = symbol_table_insert_name(&mut (*this).predicate_values, predicate_name, length);
        (*this).predicate_steps.push(TSQueryPredicateStep {
            type_: TSQueryPredicateStepTypeString,
            value_id: u32::from(id),
        });
        stream_skip_whitespace(stream);

        loop {
            if (*stream).next == i32::from(b')') {
                stream_advance(stream);
                stream_skip_whitespace(stream);
                (*this).predicate_steps.push(TSQueryPredicateStep {
                    type_: TSQueryPredicateStepTypeDone,
                    value_id: 0,
                });
                break;
            }
            // Parse an '@'-prefixed capture name
            else if (*stream).next == i32::from(b'@') {
                stream_advance(stream);

                // Parse the capture name
                if !stream_is_ident_start(stream) {
                    return TSQueryErrorSyntax;
                }
                let capture_name = (*stream).input;
                stream_scan_identifier(stream);
                let capture_length = (*stream).input.offset_from(capture_name) as u32;

                // Add the capture id to the first step of the pattern
                let capture_id =
                    symbol_table_id_for_name(&(*this).captures, capture_name, capture_length);
                if capture_id == -1 {
                    stream_reset(stream, capture_name);
                    return TSQueryErrorCapture;
                }

                (*this).predicate_steps.push(TSQueryPredicateStep {
                    type_: TSQueryPredicateStepTypeCapture,
                    value_id: capture_id as u32,
                });
            }
            // Parse a string literal
            else if (*stream).next == i32::from(b'"') {
                let e = query_parse_string_literal(this, stream);
                if e != 0 {
                    return e;
                }
                let query_id = symbol_table_insert_name(
                    &mut (*this).predicate_values,
                    (*this).string_buffer.contents,
                    (*this).string_buffer.size,
                );
                (*this).predicate_steps.push(TSQueryPredicateStep {
                    type_: TSQueryPredicateStepTypeString,
                    value_id: u32::from(query_id),
                });
            }
            // Parse a bare symbol
            else if stream_is_ident_start(stream) {
                let symbol_start = (*stream).input;
                stream_scan_identifier(stream);
                let symbol_length = (*stream).input.offset_from(symbol_start) as u32;
                let query_id = symbol_table_insert_name(
                    &mut (*this).predicate_values,
                    symbol_start,
                    symbol_length,
                );
                (*this).predicate_steps.push(TSQueryPredicateStep {
                    type_: TSQueryPredicateStepTypeString,
                    value_id: u32::from(query_id),
                });
            } else {
                return TSQueryErrorSyntax;
            }

            stream_skip_whitespace(stream);
        }

        0
    }
}

/// Read one S-expression pattern from the stream, and incorporate it into the query's internal
/// state machine representation. For nested patterns, this function calls itself recursively.
///
/// The caller is responsible for passing in a dedicated `CaptureQuantifiers`.
unsafe fn query_parse_pattern(
    this: *mut TSQuery,
    stream: *mut Stream,
    depth: u32,
    is_immediate: bool,
    is_inside_alternation: bool,
    capture_quantifiers: *mut CaptureQuantifiers,
) -> TSQueryError {
    unsafe {
        if (*stream).next == 0 {
            return TSQueryErrorSyntax;
        }
        if (*stream).next == i32::from(b')') || (*stream).next == i32::from(b']') {
            return PARENT_DONE;
        }

        let starting_step_index = (*this).steps.size;

        // Store the byte offset of each step in the query.
        if (*this).step_offsets.size == 0
            || u32::from((*(*this).step_offsets.back()).step_index) != starting_step_index
        {
            (*this).step_offsets.push(StepOffset {
                step_index: starting_step_index as u16,
                byte_offset: stream_offset(stream),
            });
        }

        // An open bracket is the start of an alternation.
        if (*stream).next == i32::from(b'[') {
            stream_advance(stream);
            stream_skip_whitespace(stream);

            // Parse each branch, and add a placeholder step in between the branches.
            let mut branch_step_indices: Array<u32> = Array::new();
            let mut branch_capture_quantifiers = capture_quantifiers_new();
            loop {
                let start_index = (*this).steps.size;
                let mut e = query_parse_pattern(
                    this,
                    stream,
                    depth,
                    is_immediate,
                    true,
                    &mut branch_capture_quantifiers,
                );

                if e == PARENT_DONE {
                    if (*stream).next == i32::from(b']') && branch_step_indices.size > 0 {
                        stream_advance(stream);
                        break;
                    }
                    e = TSQueryErrorSyntax;
                }
                if e != 0 {
                    capture_quantifiers_delete(&mut branch_capture_quantifiers);
                    branch_step_indices.delete();
                    return e;
                }

                if start_index == starting_step_index {
                    capture_quantifiers_replace(
                        capture_quantifiers,
                        &mut branch_capture_quantifiers,
                    );
                } else {
                    capture_quantifiers_join_all(
                        capture_quantifiers,
                        &mut branch_capture_quantifiers,
                    );
                }

                branch_step_indices.push(start_index);
                (*this).steps.push(query_step_new(0, depth as u16, false));
                capture_quantifiers_clear(&mut branch_capture_quantifiers);
            }
            let _ = (*this).steps.pop();

            // For all of the branches except for the last one, add the subsequent branch as an
            // alternative, and link the end of the branch to the current end of the steps.
            let mut i = 0u32;
            while i < branch_step_indices.size.wrapping_sub(1) {
                let step_index = *branch_step_indices.get(i);
                let next_step_index = *branch_step_indices.get(i + 1);
                let start_step = (*this).steps.get(step_index);
                let end_step = (*this).steps.get(next_step_index - 1);
                (*start_step).alternative_index = next_step_index as u16;
                (*end_step).alternative_index = (*this).steps.size as u16;
                (*end_step).is_dead_end = true;
                i += 1;
            }

            capture_quantifiers_delete(&mut branch_capture_quantifiers);
            branch_step_indices.delete();
        }
        // An open parenthesis can be the start of three possible constructs:
        // * A grouped sequence
        // * A predicate
        // * A named node
        else if (*stream).next == i32::from(b'(') {
            stream_advance(stream);
            stream_skip_whitespace(stream);

            // If this parenthesis is followed by a node, then it represents a grouped sequence.
            if (*stream).next == i32::from(b'(')
                || (*stream).next == i32::from(b'"')
                || (*stream).next == i32::from(b'[')
            {
                let mut child_is_immediate = is_immediate;
                let mut child_capture_quantifiers = capture_quantifiers_new();
                loop {
                    if (*stream).next == i32::from(b'.') {
                        let anchor_start = (*stream).input;
                        child_is_immediate = true;
                        stream_advance(stream);
                        stream_skip_whitespace(stream);
                        // A `.` at a group's end has no sibling to anchor, and a group is not a
                        // node, so there is no last child to anchor against.
                        if (*stream).next == i32::from(b')') {
                            stream_reset(stream, anchor_start);
                            capture_quantifiers_delete(&mut child_capture_quantifiers);
                            return TSQueryErrorSyntax;
                        }
                    }
                    let mut e = query_parse_pattern(
                        this,
                        stream,
                        depth,
                        child_is_immediate,
                        is_inside_alternation,
                        &mut child_capture_quantifiers,
                    );
                    if e == PARENT_DONE {
                        if (*stream).next == i32::from(b')') {
                            stream_advance(stream);
                            break;
                        }
                        e = TSQueryErrorSyntax;
                    }
                    if e != 0 {
                        capture_quantifiers_delete(&mut child_capture_quantifiers);
                        return e;
                    }

                    capture_quantifiers_add_all(
                        capture_quantifiers,
                        &mut child_capture_quantifiers,
                    );
                    capture_quantifiers_clear(&mut child_capture_quantifiers);
                    child_is_immediate = false;
                }

                capture_quantifiers_delete(&mut child_capture_quantifiers);
            }
            // A dot/pound character indicates the start of a predicate.
            else if (*stream).next == i32::from(b'.') || (*stream).next == i32::from(b'#') {
                stream_advance(stream);
                return query_parse_predicate(this, stream);
            }
            // Otherwise, this parenthesis is the start of a named node.
            else {
                let symbol: TSSymbol;
                let mut is_missing = false;
                let node_name = (*stream).input;

                // Parse a normal node name
                if stream_is_ident_start(stream) {
                    stream_scan_identifier(stream);
                    let length = (*stream).input.offset_from(node_name) as u32;

                    // Parse the wildcard symbol
                    if length == 1 && *node_name == b'_' as c_char {
                        symbol = WILDCARD_SYMBOL;
                    } else if length == 7
                        && strncmp(node_name, c"MISSING".as_ptr(), length as usize) == 0
                    {
                        is_missing = true;
                        stream_skip_whitespace(stream);

                        if stream_is_ident_start(stream) {
                            let missing_node_name = (*stream).input;
                            stream_scan_identifier(stream);
                            let missing_node_length =
                                (*stream).input.offset_from(missing_node_name) as u32;
                            symbol = ts_language_symbol_for_name(
                                (*this).language,
                                missing_node_name,
                                missing_node_length,
                                true,
                            );
                            if symbol == 0 {
                                stream_reset(stream, missing_node_name);
                                return TSQueryErrorNodeType;
                            }
                        } else if (*stream).next == i32::from(b'"') {
                            let string_start = (*stream).input;
                            let e = query_parse_string_literal(this, stream);
                            if e != 0 {
                                return e;
                            }

                            symbol = ts_language_symbol_for_name(
                                (*this).language,
                                (*this).string_buffer.contents,
                                (*this).string_buffer.size,
                                false,
                            );
                            if symbol == 0 {
                                stream_reset(stream, string_start.add(1));
                                return TSQueryErrorNodeType;
                            }
                        } else if (*stream).next == i32::from(b')') {
                            symbol = WILDCARD_SYMBOL;
                        } else {
                            stream_reset(stream, (*stream).input);
                            return TSQueryErrorSyntax;
                        }
                    } else {
                        symbol =
                            ts_language_symbol_for_name((*this).language, node_name, length, true);
                        if symbol == 0 {
                            stream_reset(stream, node_name);
                            return TSQueryErrorNodeType;
                        }
                    }
                } else {
                    return TSQueryErrorSyntax;
                }

                // Add a step for the node.
                (*this)
                    .steps
                    .push(query_step_new(symbol, depth as u16, is_immediate));
                let step = (*this).steps.back();
                if ts_language_symbol_metadata((*this).language, symbol).supertype {
                    (*step).supertype_symbol = (*step).symbol;
                    (*step).symbol = WILDCARD_SYMBOL;
                }
                if is_missing {
                    (*step).is_missing = true;
                }
                if symbol == WILDCARD_SYMBOL {
                    (*step).is_named = true;
                }

                // Parse a supertype symbol
                if (*stream).next == i32::from(b'/') {
                    if (*step).supertype_symbol == 0 {
                        stream_reset(stream, node_name.sub(1)); // reset to the start of the node
                        return TSQueryErrorStructure;
                    }

                    stream_advance(stream);

                    let subtype_node_name = (*stream).input;

                    if stream_is_ident_start(stream) {
                        // Named node
                        stream_scan_identifier(stream);
                        let length = (*stream).input.offset_from(subtype_node_name) as u32;
                        (*step).symbol = ts_language_symbol_for_name(
                            (*this).language,
                            subtype_node_name,
                            length,
                            true,
                        );
                    } else if (*stream).next == i32::from(b'"') {
                        // Anonymous leaf node
                        let e = query_parse_string_literal(this, stream);
                        if e != 0 {
                            return e;
                        }
                        (*step).symbol = ts_language_symbol_for_name(
                            (*this).language,
                            (*this).string_buffer.contents,
                            (*this).string_buffer.size,
                            false,
                        );
                    } else {
                        return TSQueryErrorSyntax;
                    }

                    if (*step).symbol == 0 {
                        stream_reset(stream, subtype_node_name);
                        return TSQueryErrorNodeType;
                    }

                    // Get all the possible subtypes for the given supertype,
                    // and check if the given subtype is valid.
                    if (*(*this).language).abi_version >= LANGUAGE_VERSION_WITH_RESERVED_WORDS {
                        let mut subtype_length = 0u32;
                        let subtypes = ts_language_subtypes(
                            (*this).language,
                            (*step).supertype_symbol,
                            &mut subtype_length,
                        );

                        let mut subtype_is_valid = false;
                        for i in 0..subtype_length {
                            if *subtypes.add(i as usize) == (*step).symbol {
                                subtype_is_valid = true;
                                break;
                            }
                        }

                        // This subtype is not valid for the given supertype.
                        if !subtype_is_valid {
                            stream_reset(stream, node_name.sub(1)); // reset to the start of the node
                            return TSQueryErrorStructure;
                        }
                    }
                }

                stream_skip_whitespace(stream);

                // Parse the child patterns
                let mut child_is_immediate = false;
                let mut last_child_step_index = 0u16;
                let mut negated_field_count = 0u16;
                let mut negated_field_ids = [0 as TSFieldId; MAX_NEGATED_FIELD_COUNT];
                let mut child_capture_quantifiers = capture_quantifiers_new();
                loop {
                    // Parse a negated field assertion
                    if (*stream).next == i32::from(b'!') {
                        stream_advance(stream);
                        stream_skip_whitespace(stream);
                        if !stream_is_ident_start(stream) {
                            capture_quantifiers_delete(&mut child_capture_quantifiers);
                            return TSQueryErrorSyntax;
                        }
                        let field_name = (*stream).input;
                        stream_scan_identifier(stream);
                        let length = (*stream).input.offset_from(field_name) as u32;
                        stream_skip_whitespace(stream);

                        let field_id =
                            ts_language_field_id_for_name((*this).language, field_name, length);
                        if field_id == 0 {
                            (*stream).input = field_name;
                            capture_quantifiers_delete(&mut child_capture_quantifiers);
                            return TSQueryErrorField;
                        }

                        // Keep the field ids sorted.
                        if (negated_field_count as usize) < MAX_NEGATED_FIELD_COUNT {
                            negated_field_ids[negated_field_count as usize] = field_id;
                            negated_field_count += 1;
                        }

                        continue;
                    }

                    // Parse a sibling anchor
                    if (*stream).next == i32::from(b'.') {
                        child_is_immediate = true;
                        stream_advance(stream);
                        stream_skip_whitespace(stream);
                    }

                    let mut step_index = (*this).steps.size as u16;
                    let mut e = query_parse_pattern(
                        this,
                        stream,
                        depth + 1,
                        child_is_immediate,
                        is_inside_alternation,
                        &mut child_capture_quantifiers,
                    );
                    // In the event we only parsed a predicate, meaning no new steps were added,
                    // then subtract one so we're not indexing past the end of the array
                    if u32::from(step_index) == (*this).steps.size {
                        step_index = step_index.wrapping_sub(1);
                    }
                    if e == PARENT_DONE {
                        if (*stream).next == i32::from(b')') {
                            if child_is_immediate {
                                if last_child_step_index == 0 {
                                    capture_quantifiers_delete(&mut child_capture_quantifiers);
                                    return TSQueryErrorSyntax;
                                }
                                // Mark this step *and* its alternatives as the last child of the parent.
                                let last_child_step =
                                    (*this).steps.get(u32::from(last_child_step_index));
                                (*last_child_step).is_last_child = true;
                                if (*last_child_step).alternative_index != NONE
                                    && u32::from((*last_child_step).alternative_index)
                                        < (*this).steps.size
                                {
                                    let mut alternative_step = (*this)
                                        .steps
                                        .get(u32::from((*last_child_step).alternative_index));
                                    (*alternative_step).is_last_child = true;
                                    while (*alternative_step).alternative_index != NONE
                                        && u32::from((*alternative_step).alternative_index)
                                            < (*this).steps.size
                                    {
                                        alternative_step = (*this)
                                            .steps
                                            .get(u32::from((*alternative_step).alternative_index));
                                        (*alternative_step).is_last_child = true;
                                    }
                                }
                            }

                            if negated_field_count != 0 {
                                query_add_negated_fields(
                                    this,
                                    starting_step_index as u16,
                                    negated_field_ids.as_ptr(),
                                    negated_field_count,
                                );
                            }

                            stream_advance(stream);
                            break;
                        }
                        e = TSQueryErrorSyntax;
                    }
                    if e != 0 {
                        capture_quantifiers_delete(&mut child_capture_quantifiers);
                        return e;
                    }

                    capture_quantifiers_add_all(
                        capture_quantifiers,
                        &mut child_capture_quantifiers,
                    );

                    last_child_step_index = step_index;
                    child_is_immediate = false;
                    capture_quantifiers_clear(&mut child_capture_quantifiers);
                }
                capture_quantifiers_delete(&mut child_capture_quantifiers);
            }
        }
        // Parse a wildcard pattern
        else if (*stream).next == i32::from(b'_') {
            stream_advance(stream);
            stream_skip_whitespace(stream);

            // Add a step that matches any kind of node
            (*this)
                .steps
                .push(query_step_new(WILDCARD_SYMBOL, depth as u16, is_immediate));
        }
        // Parse a double-quoted anonymous leaf node expression
        else if (*stream).next == i32::from(b'"') {
            let string_start = (*stream).input;
            let e = query_parse_string_literal(this, stream);
            if e != 0 {
                return e;
            }

            // Add a step for the node
            let symbol = ts_language_symbol_for_name(
                (*this).language,
                (*this).string_buffer.contents,
                (*this).string_buffer.size,
                false,
            );
            if symbol == 0 {
                stream_reset(stream, string_start.add(1));
                return TSQueryErrorNodeType;
            }
            (*this)
                .steps
                .push(query_step_new(symbol, depth as u16, is_immediate));
        }
        // Parse a field-prefixed pattern
        else if stream_is_ident_start(stream) {
            // Parse the field name
            let field_name = (*stream).input;
            stream_scan_identifier(stream);
            let length = (*stream).input.offset_from(field_name) as u32;
            stream_skip_whitespace(stream);

            if (*stream).next != i32::from(b':') {
                stream_reset(stream, field_name);
                return TSQueryErrorSyntax;
            }
            stream_advance(stream);
            stream_skip_whitespace(stream);

            // Parse the pattern
            let mut field_capture_quantifiers = capture_quantifiers_new();
            let mut e = query_parse_pattern(
                this,
                stream,
                depth,
                is_immediate,
                is_inside_alternation,
                &mut field_capture_quantifiers,
            );
            if e != 0 {
                capture_quantifiers_delete(&mut field_capture_quantifiers);
                if e == PARENT_DONE {
                    e = TSQueryErrorSyntax;
                }
                return e;
            }

            // Add the field name to the first step of the pattern
            let field_id = ts_language_field_id_for_name((*this).language, field_name, length);
            if field_id == 0 {
                (*stream).input = field_name;
                // (the C leaks `field_capture_quantifiers` on this path; kept identical)
                return TSQueryErrorField;
            }

            let mut step_index = starting_step_index;
            let mut step = (*this).steps.get(step_index);
            loop {
                (*step).field = field_id;
                if (*step).alternative_index != NONE
                    && u32::from((*step).alternative_index) > step_index
                    && u32::from((*step).alternative_index) < (*this).steps.size
                {
                    step_index = u32::from((*step).alternative_index);
                    step = (*this).steps.get(step_index);
                } else {
                    break;
                }
            }

            capture_quantifiers_add_all(capture_quantifiers, &mut field_capture_quantifiers);
            capture_quantifiers_delete(&mut field_capture_quantifiers);
        } else {
            return TSQueryErrorSyntax;
        }

        stream_skip_whitespace(stream);

        // Parse suffixes modifiers for this pattern
        let mut quantifier = TSQuantifierOne;
        loop {
            // Parse the one-or-more operator.
            if (*stream).next == i32::from(b'+') {
                quantifier = quantifier_join(TSQuantifierOneOrMore, quantifier);

                stream_advance(stream);
                stream_skip_whitespace(stream);
            }
            // Parse the zero-or-more repetition operator.
            else if (*stream).next == i32::from(b'*') {
                quantifier = quantifier_join(TSQuantifierZeroOrMore, quantifier);

                stream_advance(stream);
                stream_skip_whitespace(stream);
            }
            // Parse the optional operator.
            else if (*stream).next == i32::from(b'?') {
                quantifier = quantifier_join(TSQuantifierZeroOrOne, quantifier);

                stream_advance(stream);
                stream_skip_whitespace(stream);
            }
            // Parse an '@'-prefixed capture pattern
            else if (*stream).next == i32::from(b'@') {
                stream_advance(stream);
                if !stream_is_ident_start(stream) {
                    return TSQueryErrorSyntax;
                }
                let capture_name = (*stream).input;
                stream_scan_identifier(stream);
                let length = (*stream).input.offset_from(capture_name) as u32;
                stream_skip_whitespace(stream);

                // Add the capture id to the first step of the pattern
                let capture_id =
                    symbol_table_insert_name(&mut (*this).captures, capture_name, length);

                // Add the capture quantifier
                capture_quantifiers_add_for_id(capture_quantifiers, capture_id, TSQuantifierOne);

                let mut step_index = starting_step_index;
                loop {
                    let step = (*this).steps.get(step_index);
                    query_step_add_capture(&mut *step, capture_id);
                    if (*step).alternative_index != NONE
                        && u32::from((*step).alternative_index) > step_index
                        && u32::from((*step).alternative_index) < (*this).steps.size
                    {
                        step_index = u32::from((*step).alternative_index);
                    } else {
                        break;
                    }
                }
            }
            // No more suffix modifiers
            else {
                break;
            }
        }

        match quantifier {
            TSQuantifierOneOrMore => {
                let mut repeat_step = query_step_new(WILDCARD_SYMBOL, depth as u16, false);
                repeat_step.is_inside_alternation = is_inside_alternation;
                repeat_step.alternative_index = starting_step_index as u16;
                repeat_step.is_pass_through = true;
                (*this).steps.push(repeat_step);
            }
            TSQuantifierZeroOrMore => {
                let mut repeat_step = query_step_new(WILDCARD_SYMBOL, depth as u16, false);
                repeat_step.is_inside_alternation = is_inside_alternation;
                repeat_step.alternative_index = starting_step_index as u16;
                repeat_step.is_pass_through = true;
                (*this).steps.push(repeat_step);

                // Stop when `step->alternative_index` is `NONE` or it points to
                // `repeat_step` or beyond. Note that having just been pushed,
                // `repeat_step` occupies slot `self->steps.size - 1`.
                let mut step = (*this).steps.get(starting_step_index);
                while (*step).alternative_index != NONE
                    && u32::from((*step).alternative_index) < (*this).steps.size - 1
                {
                    step = (*this).steps.get(u32::from((*step).alternative_index));
                }
                (*step).alternative_index = (*this).steps.size as u16;
                (*step).alternative_is_skip = true;
            }
            TSQuantifierZeroOrOne => {
                let mut step = (*this).steps.get(starting_step_index);
                while (*step).alternative_index != NONE
                    && u32::from((*step).alternative_index) < (*this).steps.size
                {
                    step = (*this).steps.get(u32::from((*step).alternative_index));
                }
                (*step).alternative_index = (*this).steps.size as u16;
                (*step).alternative_is_skip = true;
            }
            _ => {}
        }

        capture_quantifiers_mul(capture_quantifiers, quantifier);

        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_new(
    language: *const TSLanguage,
    source: *const c_char,
    source_len: u32,
    error_offset: *mut u32,
    error_type: *mut TSQueryError,
) -> *mut TSQuery {
    unsafe {
        if language.is_null()
            || (*language).abi_version > TREE_SITTER_LANGUAGE_VERSION
            || (*language).abi_version < TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION
        {
            *error_type = TSQueryErrorLanguage;
            return ptr::null_mut();
        }

        let this = ts_malloc(size_of::<TSQuery>()) as *mut TSQuery;
        ptr::write(
            this,
            TSQuery {
                steps: Array::new(),
                pattern_map: Array::new(),
                captures: symbol_table_new(),
                capture_quantifiers: Array::new(),
                predicate_values: symbol_table_new(),
                predicate_steps: Array::new(),
                patterns: Array::new(),
                step_offsets: Array::new(),
                string_buffer: Array::new(),
                negated_fields: Array::new(),
                repeat_symbols_with_rootless_patterns: Array::new(),
                wildcard_root_pattern_count: 0,
                language: ts_language_copy(language),
            },
        );

        (*this).negated_fields.push(0);

        // Parse all of the S-expressions in the given string.
        let mut stream = stream_new(source, source_len);
        stream_skip_whitespace(&mut stream);
        while stream.input < stream.end {
            let pattern_index = (*this).patterns.size;
            let mut start_step_index = (*this).steps.size;
            let start_predicate_step_index = (*this).predicate_steps.size;
            (*this).patterns.push(QueryPattern {
                steps: Slice {
                    offset: start_step_index,
                    length: 0,
                },
                predicate_steps: Slice {
                    offset: start_predicate_step_index,
                    length: 0,
                },
                start_byte: stream_offset(&stream),
                end_byte: 0,
                is_non_local: false,
            });
            let mut capture_quantifiers = capture_quantifiers_new();
            *error_type =
                query_parse_pattern(this, &mut stream, 0, false, false, &mut capture_quantifiers);
            (*this)
                .steps
                .push(query_step_new(0, PATTERN_DONE_MARKER, false));

            let pattern = (*this).patterns.back();
            (*pattern).steps.length = (*this).steps.size - start_step_index;
            (*pattern).predicate_steps.length =
                (*this).predicate_steps.size - start_predicate_step_index;
            (*pattern).end_byte = stream_offset(&stream);

            // If any pattern could not be parsed, then report the error information
            // and terminate.
            if *error_type != 0 {
                if *error_type == PARENT_DONE {
                    *error_type = TSQueryErrorSyntax;
                }
                *error_offset = stream_offset(&stream);
                capture_quantifiers_delete(&mut capture_quantifiers);
                ts_query_delete(this);
                return ptr::null_mut();
            }

            // Maintain a list of capture quantifiers for each pattern
            (*this).capture_quantifiers.push(capture_quantifiers);

            // Maintain a map that can look up patterns for a given root symbol.
            let mut wildcard_root_alternative_index = NONE;
            loop {
                let mut step = (*this).steps.get(start_step_index);

                // If a pattern has a wildcard at its root, but it has a non-wildcard child,
                // then optimize the matching process by skipping matching the wildcard.
                // Later, during the matching process, the query cursor will check that
                // there is a parent node, and capture it if necessary.
                if (*step).symbol == WILDCARD_SYMBOL && (*step).depth == 0 && (*step).field == 0 {
                    let second_step = (*this).steps.get(start_step_index + 1);
                    if (*second_step).symbol != WILDCARD_SYMBOL
                        && (*second_step).depth == 1
                        && !(*second_step).is_immediate
                    {
                        wildcard_root_alternative_index = (*step).alternative_index;
                        start_step_index += 1;
                        step = second_step;
                    }
                }

                // Determine whether the pattern has a single root node. This affects
                // decisions about whether or not to start matching the pattern when
                // a query cursor has a range restriction or when immediately within an
                // error node.
                let start_depth = (*step).depth;
                let mut is_rooted = start_depth == 0;
                let mut step_index = start_step_index + 1;
                while step_index < (*this).steps.size {
                    let child_step = (*this).steps.get(step_index);
                    if (*child_step).is_dead_end {
                        break;
                    }
                    if (*child_step).depth == start_depth {
                        is_rooted = false;
                        break;
                    }
                    step_index += 1;
                }

                query_pattern_map_insert(
                    this,
                    (*step).symbol,
                    PatternEntry {
                        step_index: start_step_index as u16,
                        pattern_index: pattern_index as u16,
                        is_rooted,
                    },
                );
                if (*step).symbol == WILDCARD_SYMBOL {
                    (*this).wildcard_root_pattern_count =
                        (*this).wildcard_root_pattern_count.wrapping_add(1);
                }

                // If there are alternatives or options at the root of the pattern,
                // then add multiple entries to the pattern map.
                if (*step).alternative_index != NONE {
                    start_step_index = u32::from((*step).alternative_index);
                } else if wildcard_root_alternative_index != NONE {
                    start_step_index = u32::from(wildcard_root_alternative_index);
                    wildcard_root_alternative_index = NONE;
                } else {
                    break;
                }
            }

            // Fix up quantifier loop-backs within alternations. When a branch of an
            // alternation has a + or * quantifier, the quantifier's pass_through step
            // loops back to the branch's first step. However, the alternation linking
            // assigns that same step's `alternative_index` to point to the _next_ branch.
            // This causes the quantifier loop to incorrectly explore other alternation branches,
            // when a quantified branch matches, loops back, and then fails to match. To correct
            // this, we create "clean" copies of the branches' first steps without the link to the
            // next branch. After a quantified branch matches, it loops back to the cleaned copy.
            {
                let pat_start = (*pattern).steps.offset;
                let pat_end = pat_start
                    .wrapping_add((*pattern).steps.length)
                    .wrapping_sub(1); // exclude DONE

                let mut i = pat_start;
                while i < pat_end {
                    let s = (*this).steps.get(i);
                    // Ensure this step is a pass_through with a _backward_ alternative (a quantifier loop-back)
                    if !(*s).is_pass_through
                        || !(*s).is_inside_alternation
                        || (*s).alternative_index == NONE
                        || u32::from((*s).alternative_index) >= i
                    {
                        i += 1;
                        continue;
                    }

                    let target_idx = u32::from((*s).alternative_index);
                    let target = (*this).steps.get(target_idx);

                    // Check if the target has a forward alternative from alternation linking
                    let target_alt_index = (*target).alternative_index;
                    if target_alt_index == NONE
                        || u32::from(target_alt_index) <= target_idx
                        || u32::from(target_alt_index) >= pat_end
                    {
                        i += 1;
                        continue;
                    }

                    // Create a clean copy of the target step without the alternation alternative.
                    let copy_idx = (*this).steps.size;
                    let mut copy = *target;
                    copy.alternative_index = NONE;
                    let target_depth = (*target).depth;
                    (*this).steps.push(copy);

                    // Add a dead_end that redirects to the pass through step after the target,
                    // so the pattern continues correctly after the cleaned copy matches.
                    let mut redirect = query_step_new(0, target_depth, false);
                    redirect.is_dead_end = true;
                    redirect.alternative_index = (target_idx + 1) as u16;
                    (*this).steps.push(redirect);

                    // Update the pass_through to loop back to the copy. Reacquire `s` since
                    // `self->steps` may have been reallocated.
                    let s = (*this).steps.get(i);
                    (*s).alternative_index = copy_idx as u16;
                    i += 1;
                }
            }
        }

        if !query_analyze_patterns(this, error_offset) {
            *error_type = TSQueryErrorStructure;
            ts_query_delete(this);
            return ptr::null_mut();
        }

        (*this).string_buffer.delete();
        this
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_delete(this: *mut TSQuery) {
    unsafe {
        if !this.is_null() {
            (*this).steps.delete();
            (*this).pattern_map.delete();
            (*this).predicate_steps.delete();
            (*this).patterns.delete();
            (*this).step_offsets.delete();
            (*this).string_buffer.delete();
            (*this).negated_fields.delete();
            (*this).repeat_symbols_with_rootless_patterns.delete();
            ts_language_delete((*this).language);
            symbol_table_delete(&mut (*this).captures);
            symbol_table_delete(&mut (*this).predicate_values);
            for index in 0..(*this).capture_quantifiers.size {
                let capture_quantifiers = (*this).capture_quantifiers.get(index);
                capture_quantifiers_delete(capture_quantifiers);
            }
            (*this).capture_quantifiers.delete();
            ts_free(this as *mut c_void);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_copy(this: *const TSQuery) -> *mut TSQuery {
    unsafe {
        let copy = ts_malloc(size_of::<TSQuery>()) as *mut TSQuery;
        ptr::write(
            copy,
            TSQuery {
                captures: symbol_table_new(),
                predicate_values: symbol_table_new(),
                capture_quantifiers: Array::new(),
                steps: Array::new(),
                pattern_map: Array::new(),
                predicate_steps: Array::new(),
                patterns: Array::new(),
                step_offsets: Array::new(),
                negated_fields: Array::new(),
                string_buffer: Array::new(),
                repeat_symbols_with_rootless_patterns: Array::new(),
                language: ts_language_copy((*this).language),
                wildcard_root_pattern_count: (*this).wildcard_root_pattern_count,
            },
        );

        (*copy).steps.assign(&(*this).steps);
        (*copy).pattern_map.assign(&(*this).pattern_map);
        (*copy).predicate_steps.assign(&(*this).predicate_steps);
        (*copy).patterns.assign(&(*this).patterns);
        (*copy).step_offsets.assign(&(*this).step_offsets);
        (*copy).negated_fields.assign(&(*this).negated_fields);
        (*copy).string_buffer.assign(&(*this).string_buffer);
        (*copy)
            .repeat_symbols_with_rootless_patterns
            .assign(&(*this).repeat_symbols_with_rootless_patterns);
        (*copy)
            .captures
            .characters
            .assign(&(*this).captures.characters);
        (*copy).captures.slices.assign(&(*this).captures.slices);
        (*copy)
            .predicate_values
            .characters
            .assign(&(*this).predicate_values.characters);
        (*copy)
            .predicate_values
            .slices
            .assign(&(*this).predicate_values.slices);

        (*copy)
            .capture_quantifiers
            .assign(&(*this).capture_quantifiers);
        for i in 0..(*copy).capture_quantifiers.size {
            let dst = (*copy).capture_quantifiers.get(i);
            let src = (*this).capture_quantifiers.get(i);
            *dst = capture_quantifiers_new();
            (*dst).assign(&*src);
        }

        copy
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_pattern_count(this: *const TSQuery) -> u32 {
    unsafe { (*this).patterns.size }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_capture_count(this: *const TSQuery) -> u32 {
    unsafe { (*this).captures.slices.size }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_string_count(this: *const TSQuery) -> u32 {
    unsafe { (*this).predicate_values.slices.size }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_capture_name_for_id(
    this: *const TSQuery,
    index: u32,
    length: *mut u32,
) -> *const c_char {
    unsafe { symbol_table_name_for_id(&(*this).captures, index as u16, length) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_capture_quantifier_for_id(
    this: *const TSQuery,
    pattern_index: u32,
    capture_index: u32,
) -> TSQuantifier {
    unsafe {
        let capture_quantifiers = (*this).capture_quantifiers.get(pattern_index);
        capture_quantifier_for_id(capture_quantifiers, capture_index as u16)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_string_value_for_id(
    this: *const TSQuery,
    index: u32,
    length: *mut u32,
) -> *const c_char {
    unsafe { symbol_table_name_for_id(&(*this).predicate_values, index as u16, length) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_predicates_for_pattern(
    this: *const TSQuery,
    pattern_index: u32,
    step_count: *mut u32,
) -> *const TSQueryPredicateStep {
    unsafe {
        let slice = (*(*this).patterns.get(pattern_index)).predicate_steps;
        *step_count = slice.length;
        if slice.length == 0 {
            return ptr::null();
        }
        (*this).predicate_steps.get(slice.offset)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_start_byte_for_pattern(
    this: *const TSQuery,
    pattern_index: u32,
) -> u32 {
    unsafe { (*(*this).patterns.get(pattern_index)).start_byte }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_end_byte_for_pattern(
    this: *const TSQuery,
    pattern_index: u32,
) -> u32 {
    unsafe { (*(*this).patterns.get(pattern_index)).end_byte }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_is_pattern_rooted(
    this: *const TSQuery,
    pattern_index: u32,
) -> bool {
    unsafe {
        for i in 0..(*this).pattern_map.size {
            let entry = (*this).pattern_map.get(i);
            if u32::from((*entry).pattern_index) == pattern_index && !(*entry).is_rooted {
                return false;
            }
        }
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_is_pattern_non_local(
    this: *const TSQuery,
    pattern_index: u32,
) -> bool {
    unsafe {
        if pattern_index < (*this).patterns.size {
            (*(*this).patterns.get(pattern_index)).is_non_local
        } else {
            false
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_is_pattern_guaranteed_at_step(
    this: *const TSQuery,
    byte_offset: u32,
) -> bool {
    unsafe {
        let mut step_index = u32::MAX;
        for i in 0..(*this).step_offsets.size {
            let step_offset = (*this).step_offsets.get(i);
            if (*step_offset).byte_offset > byte_offset {
                break;
            }
            step_index = u32::from((*step_offset).step_index);
        }
        if step_index < (*this).steps.size {
            (*(*this).steps.get(step_index)).root_pattern_guaranteed
        } else {
            false
        }
    }
}

pub unsafe fn query_step_is_fallible(this: *const TSQuery, step_index: u16) -> bool {
    unsafe {
        let mut i = 1u32;
        let step = (*this).steps.get(u32::from(step_index));
        let mut next_step;
        loop {
            ts_assert!(u32::from(step_index) + i < (*this).steps.size);
            next_step = (*this).steps.get(u32::from(step_index) + i);
            i += 1;
            if !(*next_step).is_pass_through {
                break;
            }
        }
        (*next_step).depth != PATTERN_DONE_MARKER
            && ((*next_step).depth > (*step).depth
                || ((*next_step).depth == (*step).depth && (*next_step).is_immediate))
            && (!(*next_step).parent_pattern_guaranteed || (*step).symbol == WILDCARD_SYMBOL)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_disable_capture(
    this: *mut TSQuery,
    name: *const c_char,
    length: u32,
) {
    unsafe {
        // Remove capture information for any pattern step that previously
        // captured with the given name.
        let id = symbol_table_id_for_name(&(*this).captures, name, length);
        if id != -1 {
            for i in 0..(*this).steps.size {
                let step = (*this).steps.get(i);
                query_step_remove_capture(&mut *step, id as u16);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_disable_pattern(this: *mut TSQuery, pattern_index: u32) {
    unsafe {
        // Remove the given pattern from the pattern map. Its steps will still
        // be in the `steps` array, but they will never be read.
        let mut i = 0u32;
        while i < (*this).pattern_map.size {
            let pattern = (*this).pattern_map.get(i);
            if u32::from((*pattern).pattern_index) == pattern_index {
                (*this).pattern_map.erase(i);
                continue;
            }
            i += 1;
        }
    }
}
