//! Objects, counts and releases, driven the way compiled code drives them.

use crate::array::Arr;
use crate::array::*;
use crate::live::live_objects;
use crate::record::*;
use crate::release::*;
use crate::shape::{ARRAY, RECORD, Shape, Slot};
use crate::string::*;

fn text(s: *const Str) -> String {
    unsafe { Str::text(s) }.to_string()
}

fn rc(obj: *const u8) -> i64 {
    unsafe { *(obj as *const i64) }
}

#[test]
fn strings_are_freed_with_their_last_reference() {
    let before = live_objects();
    let shape = Shape::string();
    let s = Str::new("héllo");
    assert_eq!(live_objects(), before + 1);
    unsafe {
        dup(s as u64);
        assert_eq!(rc(s as *const u8), 2);
        release(s as *mut u8, &shape);
        assert_eq!(text(s), "héllo");
        assert_eq!(grenat_str_length(s), 5);
        release(s as *mut u8, &shape);
    }
    assert_eq!(live_objects(), before);
}

#[test]
fn adding_to_a_unique_string_appends_in_place() {
    let before = live_objects();
    let shape = Shape::string();
    unsafe {
        let a = Str::new("ab");
        let b = Str::new("cd");
        let r = grenat_str_add_owned(a, b);
        assert_eq!(r, a, "unique: reused");
        assert_eq!(text(r), "abcd");

        dup(r as u64);
        let copy = grenat_str_add_owned(r, b);
        assert_ne!(copy, r, "shared: copied");
        assert_eq!((text(copy), text(r), rc(r as *const u8)), ("abcdcd".into(), "abcd".into(), 1));
        for s in [r, b, copy] {
            release(s as *mut u8, &shape);
        }
    }
    assert_eq!(live_objects(), before);
}

#[test]
fn string_methods_follow_the_interpreter() {
    let shape = Shape::string();
    unsafe {
        let s = Str::new("  Grenat é ");
        let t = grenat_str_strip(s);
        assert_eq!(text(t), "Grenat é");
        let u = grenat_str_upcase(t);
        assert_eq!(text(u), "GRENAT É");
        let last = grenat_str_char_at(t, -1);
        assert_eq!(text(last), "é");
        assert!(grenat_str_char_at(t, 8).is_null());
        assert!(grenat_str_repeat(t, -1).is_null());
        assert_eq!(grenat_str_cmp(t, u), 1);
        let n = Str::new(" 42 ");
        assert_eq!((grenat_str_to_i(n), grenat_str_to_f(n)), (42, 42.0));
        let built = grenat_str_from(b"x=".as_ptr(), 2);
        grenat_str_push_float(built, 2.0);
        grenat_str_push_bool(built, 1);
        assert_eq!(text(built), "x=2.0true");
        for s in [s, t, u, last, n, built] {
            release(s as *mut u8, &shape);
        }
    }
}

#[test]
fn freeing_an_array_releases_its_elements() {
    let before = live_objects();
    let str_shape = Shape::string();
    let elem: [Slot; 1] = [&str_shape];
    // SAFETY: `elem` outlives `shape`
    let shape = unsafe { Shape::from_raw(ARRAY, elem.as_ptr(), 1) };
    unsafe {
        let shared = Str::new("shared");
        let a = grenat_array_new(0);
        for _ in 0..3 {
            dup(shared as u64);
            grenat_array_push(a, shared as u64);
        }
        let b = grenat_array_copy(a, 1);
        assert_eq!(rc(shared as *const u8), 7);
        let c = grenat_array_concat(a, b, 1);
        assert_eq!(Arr::items(c).len(), 6);
        for arr in [a, b, c] {
            release(arr as *mut u8, &shape);
        }
        assert_eq!(rc(shared as *const u8), 1);
        release(shared as *mut u8, &str_shape);
    }
    assert_eq!(live_objects(), before);
}

#[test]
fn a_dying_record_hands_its_memory_over() {
    let before = live_objects();
    let str_shape = Shape::string();
    let fields: [Slot; 2] = [std::ptr::null(), &str_shape];
    // SAFETY: `fields` outlives `shape`
    let shape = unsafe { Shape::from_raw(RECORD, fields.as_ptr(), 2) };
    unsafe {
        let r = grenat_record_alloc(2);
        Record::set(r, 0, 7);
        Record::set(r, 1, Str::new("name") as u64);

        dup(r as u64);
        assert!(grenat_drop_reuse(r as *mut u8, &shape).is_null(), "shared: not reusable");
        let token = grenat_drop_reuse(r as *mut u8, &shape);
        assert_eq!(token, r as *mut u8, "last reference: reusable");
        assert_eq!(live_objects(), before + 1, "the field is released, the memory kept");
        grenat_free_token(token, &shape);
        grenat_free_token(std::ptr::null_mut(), &shape);
    }
    assert_eq!(live_objects(), before);
}

#[test]
fn values_print_as_the_interpreter_prints_them() {
    use crate::write::{INSPECT, NEWLINE, PRINT, PUTS, text};
    let before = live_objects();
    let str_shape = Shape::string();
    let fields: [Slot; 2] = [std::ptr::null(), &str_shape];
    // SAFETY: `fields` outlives `shape`
    let shape = unsafe { Shape::from_raw(RECORD, fields.as_ptr(), 2) };
    let elem: [Slot; 1] = [&shape];
    // SAFETY: `elem` outlives `array`
    let array = unsafe { Shape::from_raw(ARRAY, elem.as_ptr(), 1) };
    unsafe {
        let record = |x: f64, name: &str| {
            let r = grenat_record_alloc(2);
            Record::set(r, 0, x.to_bits());
            Record::set(r, 1, Str::new(name) as u64);
            r as u64
        };
        let items = Arr::new(vec![record(1.0, "a\"b"), record(2.5, "c")]) as u64;
        // an array of records `P(x: Float, name: String)`
        let desc = "ARP(x:F,name:S)";
        assert_eq!(text(items, desc, INSPECT), "[P(x: 1.0, name: \"a\\\"b\"), P(x: 2.5, name: \"c\")]\n");
        assert_eq!(text(items, desc, PUTS), "P(x: 1.0, name: \"a\\\"b\")\nP(x: 2.5, name: \"c\")\n");
        let s = Str::new("é");
        assert_eq!((text(s as u64, "S", PRINT), text(s as u64, "S", PUTS)), ("é".into(), "é\n".into()));
        assert_eq!(text((-3i64) as u64, "I", PUTS), "-3\n");
        assert_eq!(text(1, "B", INSPECT), "true\n");
        assert_eq!(text(0, "", NEWLINE), "\n");
        release(items as *mut u8, &array);
        release(s as *mut u8, &str_shape);
    }
    assert_eq!(live_objects(), before);
}
