//! `language.c` and the helpers of `language.h` against the C originals, on every state and symbol
//! of the real grammars shipped by tree-sitter-lite-rs.
use core::ffi::{CStr, c_char};
use tree_sitter_runtime_rs::{api::*, language::*, parser_abi::*};

unsafe extern "C" {
    fn c_lookup(l: *const TSLanguage, s: u16, y: u16) -> u16;
    fn c_has_actions(l: *const TSLanguage, s: u16, y: u16) -> bool;
    fn c_actions(l: *const TSLanguage, s: u16, y: u16, n: *mut u32) -> *const TSParseAction;
    fn c_has_reduce(l: *const TSLanguage, s: u16, y: u16) -> bool;
    fn c_state_is_primary(l: *const TSLanguage, s: u16) -> bool;
    fn c_enabled_external_tokens(l: *const TSLanguage, s: u32) -> *const bool;
    fn c_alias_sequence(l: *const TSLanguage, p: u32) -> *const u16;
    fn c_alias_at(l: *const TSLanguage, p: u32, c: u32) -> u16;
    fn c_field_map(
        l: *const TSLanguage,
        p: u32,
        a: *mut *const TSFieldMapEntry,
        b: *mut *const TSFieldMapEntry,
    );
    fn c_aliases_for_symbol(l: *const TSLanguage, o: u16, a: *mut *const u16, b: *mut *const u16);
    fn c_lookaheads(l: *const TSLanguage, s: u16, out: *mut LookaheadIterator);
    fn c_lookahead_next(it: *mut LookaheadIterator) -> bool;

    fn c_ref_language_copy(l: *const TSLanguage) -> *const TSLanguage;
    fn c_ref_language_symbol_count(l: *const TSLanguage) -> u32;
    fn c_ref_language_state_count(l: *const TSLanguage) -> u32;
    fn c_ref_language_supertypes(l: *const TSLanguage, n: *mut u32) -> *const u16;
    fn c_ref_language_subtypes(l: *const TSLanguage, s: u16, n: *mut u32) -> *const u16;
    fn c_ref_language_abi_version(l: *const TSLanguage) -> u32;
    fn c_ref_language_metadata(l: *const TSLanguage) -> *const TSLanguageMetadata;
    fn c_ref_language_name(l: *const TSLanguage) -> *const c_char;
    fn c_ref_language_field_count(l: *const TSLanguage) -> u32;
    fn c_ref_language_table_entry(l: *const TSLanguage, s: u16, y: u16, r: *mut TableEntry);
    fn c_ref_language_lex_mode_for_state(l: *const TSLanguage, s: u16) -> TSLexerMode;
    fn c_ref_language_is_reserved_word(l: *const TSLanguage, s: u16, y: u16) -> bool;
    fn c_ref_language_symbol_metadata(l: *const TSLanguage, y: u16) -> TSSymbolMetadata;
    fn c_ref_language_public_symbol(l: *const TSLanguage, y: u16) -> u16;
    fn c_ref_language_next_state(l: *const TSLanguage, s: u16, y: u16) -> u16;
    fn c_ref_language_symbol_name(l: *const TSLanguage, y: u16) -> *const c_char;
    fn c_ref_language_symbol_for_name(
        l: *const TSLanguage,
        s: *const c_char,
        n: u32,
        named: bool,
    ) -> u16;
    fn c_ref_language_symbol_type(l: *const TSLanguage, y: u16) -> TSSymbolType;
    fn c_ref_language_field_name_for_id(l: *const TSLanguage, id: u16) -> *const c_char;
    fn c_ref_language_field_id_for_name(l: *const TSLanguage, s: *const c_char, n: u32) -> u16;
    fn c_ref_lookahead_iterator_new(l: *const TSLanguage, s: u16) -> *mut TSLookaheadIterator;
    fn c_ref_lookahead_iterator_delete(it: *mut TSLookaheadIterator);
    fn c_ref_lookahead_iterator_reset_state(it: *mut TSLookaheadIterator, s: u16) -> bool;
    fn c_ref_lookahead_iterator_language(it: *const TSLookaheadIterator) -> *const TSLanguage;
    fn c_ref_lookahead_iterator_reset(
        it: *mut TSLookaheadIterator,
        l: *const TSLanguage,
        s: u16,
    ) -> bool;
    fn c_ref_lookahead_iterator_next(it: *mut TSLookaheadIterator) -> bool;
    fn c_ref_lookahead_iterator_current_symbol(it: *const TSLookaheadIterator) -> u16;
    fn c_ref_lookahead_iterator_current_symbol_name(
        it: *const TSLookaheadIterator,
    ) -> *const c_char;
}

fn languages() -> Vec<(&'static str, *const TSLanguage)> {
    use tree_sitter_lite_rs as g;
    let f = |l: tree_sitter_language::LanguageFn| unsafe { (l.into_raw())() as *const TSLanguage };
    vec![
        ("javascript", f(g::javascript::LANGUAGE)),
        ("typescript", f(g::typescript::LANGUAGE_TYPESCRIPT)),
        ("tsx", f(g::tsx::LANGUAGE)),
        ("python", f(g::python::LANGUAGE)),
        ("rust", f(g::rust::LANGUAGE)),
        ("go", f(g::go::LANGUAGE)),
        ("java", f(g::java::LANGUAGE)),
        ("c", f(g::c::LANGUAGE)),
    ]
}

fn cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

/// Everything about an iterator that the C exposes, as comparable values.
type It = (u16, u16, u16, bool, usize, u16, u16, usize);
fn it_state(it: &LookaheadIterator) -> It {
    (
        it.symbol,
        it.table_value,
        it.group_count,
        it.is_small_state,
        it.actions as usize,
        it.next_state,
        it.action_count,
        it.data as usize,
    )
}

#[test]
fn tables_and_lookups_match_c() {
    for (name, l) in languages() {
        let lang = unsafe { &*l };
        let states = lang.state_count as u16;
        let symbols = (lang.symbol_count + lang.alias_count) as u16;
        unsafe {
            assert_eq!(
                ts_language_symbol_count(l),
                c_ref_language_symbol_count(l),
                "{name}"
            );
            assert_eq!(ts_language_state_count(l), c_ref_language_state_count(l));
            assert_eq!(ts_language_abi_version(l), c_ref_language_abi_version(l));
            assert_eq!(ts_language_field_count(l), c_ref_language_field_count(l));
            assert_eq!(ts_language_metadata(l), c_ref_language_metadata(l));
            assert_eq!(ts_language_name(l), c_ref_language_name(l));
            assert_eq!(ts_language_copy(l), c_ref_language_copy(l));
            ts_language_delete(l);
            let (mut a, mut b) = (0u32, 0u32);
            assert_eq!(
                ts_language_supertypes(l, &mut a),
                c_ref_language_supertypes(l, &mut b)
            );
            assert_eq!(a, b);

            for s in 0..states {
                assert_eq!(ts_language_state_is_primary(l, s), c_state_is_primary(l, s));
                let (m1, m2) = (
                    ts_language_lex_mode_for_state(l, s),
                    c_ref_language_lex_mode_for_state(l, s),
                );
                assert_eq!(
                    (m1.lex_state, m1.external_lex_state, m1.reserved_word_set_id),
                    (m2.lex_state, m2.external_lex_state, m2.reserved_word_set_id)
                );
                for y in (0..symbols).chain([ts_builtin_sym_error, ts_builtin_sym_error_repeat]) {
                    assert_eq!(
                        ts_language_next_state(l, s, y),
                        c_ref_language_next_state(l, s, y),
                        "{name} next_state {s} {y}"
                    );
                    if y < symbols {
                        assert_eq!(
                            ts_language_lookup(l, s, y),
                            c_lookup(l, s, y),
                            "{name} lookup {s} {y}"
                        );
                        assert_eq!(ts_language_has_actions(l, s, y), c_has_actions(l, s, y));
                    }
                    if u32::from(y) < lang.token_count || y >= ts_builtin_sym_error_repeat {
                        let (mut e1, mut e2) = (
                            core::mem::zeroed::<TableEntry>(),
                            core::mem::zeroed::<TableEntry>(),
                        );
                        ts_language_table_entry(l, s, y, &mut e1);
                        c_ref_language_table_entry(l, s, y, &mut e2);
                        assert_eq!(
                            (e1.actions, e1.action_count, e1.is_reusable),
                            (e2.actions, e2.action_count, e2.is_reusable),
                            "{name} table_entry {s} {y}"
                        );
                        let (mut n1, mut n2) = (0u32, 0u32);
                        assert_eq!(
                            ts_language_actions(l, s, y, &mut n1),
                            c_actions(l, s, y, &mut n2)
                        );
                        assert_eq!(n1, n2);
                        assert_eq!(
                            ts_language_has_reduce_action(l, s, y),
                            c_has_reduce(l, s, y)
                        );
                    }
                }
                for y in [0u16, 1, 2, 5, symbols / 2, symbols.saturating_sub(1)] {
                    assert_eq!(
                        ts_language_is_reserved_word(l, s, y),
                        c_ref_language_is_reserved_word(l, s, y)
                    );
                }
            }

            for y in (0..symbols + 3).chain([ts_builtin_sym_error, ts_builtin_sym_error_repeat]) {
                if y < symbols || y >= ts_builtin_sym_error_repeat {
                    let (m1, m2) = (
                        ts_language_symbol_metadata(l, y),
                        c_ref_language_symbol_metadata(l, y),
                    );
                    assert_eq!(
                        (m1.visible, m1.named, m1.supertype),
                        (m2.visible, m2.named, m2.supertype)
                    );
                    assert_eq!(
                        ts_language_symbol_type(l, y),
                        c_ref_language_symbol_type(l, y)
                    );
                    assert_eq!(
                        ts_language_public_symbol(l, y),
                        c_ref_language_public_symbol(l, y)
                    );
                }
                assert_eq!(
                    cstr(ts_language_symbol_name(l, y)),
                    cstr(c_ref_language_symbol_name(l, y)),
                    "{name} name {y}"
                );
                if y < symbols {
                    let (mut a, mut b) = (0u32, 0u32);
                    assert_eq!(
                        ts_language_subtypes(l, y, &mut a),
                        c_ref_language_subtypes(l, y, &mut b)
                    );
                    assert_eq!(a, b);
                    let (mut s1, mut e1, mut s2, mut e2) = (ptr0(), ptr0(), ptr0(), ptr0());
                    ts_language_aliases_for_symbol(l, y, &mut s1, &mut e1);
                    c_aliases_for_symbol(l, y, &mut s2, &mut e2);
                    assert_eq!((s1, e1), (s2, e2), "{name} aliases_for_symbol {y}");
                }
            }

            // Names, as the API is used: every symbol name looked up as named and anonymous.
            let mut probes: Vec<Vec<u8>> = (0..symbols)
                .filter_map(|y| cstr(ts_language_symbol_name(l, y)))
                .map(String::into_bytes)
                .collect();
            probes.extend([
                b"ERROR".to_vec(),
                b"ERRO".to_vec(),
                b"".to_vec(),
                b"_ERROR".to_vec(),
                b"zzzz".to_vec(),
                b"identifier".to_vec(),
                b"(".to_vec(),
            ]);
            for p in &probes {
                let z = std::ffi::CString::new(p.clone()).unwrap();
                for named in [false, true] {
                    // Also a prefix and a length that is too long by one.
                    for len in [p.len(), p.len().saturating_sub(1), p.len() + 1] {
                        let (a, b) = (
                            ts_language_symbol_for_name(l, z.as_ptr(), len as u32, named),
                            c_ref_language_symbol_for_name(l, z.as_ptr(), len as u32, named),
                        );
                        assert_eq!(
                            a,
                            b,
                            "{name} symbol_for_name {:?} len {len} named {named}",
                            String::from_utf8_lossy(p)
                        );
                    }
                }
            }

            let fields = lang.field_count as u16;
            for id in 0..fields + 3 {
                assert_eq!(
                    cstr(ts_language_field_name_for_id(l, id)),
                    cstr(c_ref_language_field_name_for_id(l, id))
                );
            }
            let mut fprobes: Vec<Vec<u8>> = (1..=fields)
                .filter_map(|i| cstr(ts_language_field_name_for_id(l, i)))
                .map(String::into_bytes)
                .collect();
            fprobes.extend([
                b"".to_vec(),
                b"zzzz".to_vec(),
                b"a".to_vec(),
                b"name".to_vec(),
            ]);
            for p in &fprobes {
                let z = std::ffi::CString::new(p.clone()).unwrap();
                for len in [p.len(), p.len().saturating_sub(1), p.len() + 1] {
                    assert_eq!(
                        ts_language_field_id_for_name(l, z.as_ptr(), len as u32),
                        c_ref_language_field_id_for_name(l, z.as_ptr(), len as u32),
                        "{name} field {:?} len {len}",
                        String::from_utf8_lossy(p)
                    );
                }
            }

            for p in 0..lang.production_id_count {
                assert_eq!(ts_language_alias_sequence(l, p), c_alias_sequence(l, p));
                for c in 0..u32::from(lang.max_alias_sequence_length) {
                    assert_eq!(ts_language_alias_at(l, p, c), c_alias_at(l, p, c));
                }
                let (mut s1, mut e1, mut s2, mut e2) = (
                    core::ptr::null(),
                    core::ptr::null(),
                    core::ptr::null(),
                    core::ptr::null(),
                );
                ts_language_field_map(l, p, &mut s1, &mut e1);
                c_field_map(l, p, &mut s2, &mut e2);
                assert_eq!((s1, e1), (s2, e2), "{name} field_map {p}");
            }
            for e in 0..64u32 {
                if lang.external_token_count > 0 {
                    assert_eq!(
                        ts_language_enabled_external_tokens(l, e),
                        c_enabled_external_tokens(l, e)
                    );
                }
            }
        }
    }
}

fn ptr0() -> *const u16 {
    core::ptr::null()
}

#[test]
fn lookahead_iterators_match_c() {
    for (name, l) in languages() {
        let lang = unsafe { &*l };
        for s in 0..lang.state_count as u16 {
            unsafe {
                let mut a = ts_language_lookaheads(l, s);
                let mut b = core::mem::zeroed::<LookaheadIterator>();
                c_lookaheads(l, s, &mut b);
                assert_eq!(it_state(&a), it_state(&b), "{name} init {s}");
                assert_eq!(
                    (a.language, a.group_end, a.phase),
                    (b.language, b.group_end, b.phase),
                    "{name} init fields {s}"
                );
                let mut n = 0;
                loop {
                    let (ra, rb) = (
                        ts_lookahead_iterator_next_inner(&mut a),
                        c_lookahead_next(&mut b),
                    );
                    assert_eq!(ra, rb, "{name} state {s} step {n}");
                    assert_eq!(it_state(&a), it_state(&b), "{name} state {s} step {n}");
                    if !ra {
                        break;
                    }
                    n += 1;
                }

                // The public API, on every 7th state to keep the test quick.
                if s % 7 == 0 {
                    let (ia, ib) = (
                        ts_lookahead_iterator_new(l, s),
                        c_ref_lookahead_iterator_new(l, s),
                    );
                    assert_eq!(
                        ts_lookahead_iterator_language(ia),
                        c_ref_lookahead_iterator_language(ib)
                    );
                    loop {
                        let (ra, rb) = (
                            ts_lookahead_iterator_next(ia),
                            c_ref_lookahead_iterator_next(ib),
                        );
                        assert_eq!(ra, rb);
                        if !ra {
                            break;
                        }
                        assert_eq!(
                            ts_lookahead_iterator_current_symbol(ia),
                            c_ref_lookahead_iterator_current_symbol(ib)
                        );
                        assert_eq!(
                            cstr(ts_lookahead_iterator_current_symbol_name(ia)),
                            cstr(c_ref_lookahead_iterator_current_symbol_name(ib))
                        );
                    }
                    let t = (s + 1) % lang.state_count as u16;
                    assert_eq!(
                        ts_lookahead_iterator_reset_state(ia, t),
                        c_ref_lookahead_iterator_reset_state(ib, t)
                    );
                    assert_eq!(
                        ts_lookahead_iterator_reset(ia, l, t),
                        c_ref_lookahead_iterator_reset(ib, l, t)
                    );
                    assert_eq!(
                        ts_lookahead_iterator_next(ia),
                        c_ref_lookahead_iterator_next(ib)
                    );
                    // Out-of-range states are refused.
                    let bad = lang.state_count as u16;
                    assert_eq!(
                        ts_lookahead_iterator_reset_state(ia, bad),
                        c_ref_lookahead_iterator_reset_state(ib, bad)
                    );
                    assert!(
                        ts_lookahead_iterator_new(l, bad).is_null()
                            && c_ref_lookahead_iterator_new(l, bad).is_null()
                    );
                    ts_lookahead_iterator_delete(ia);
                    c_ref_lookahead_iterator_delete(ib);
                }
            }
        }
    }
}
