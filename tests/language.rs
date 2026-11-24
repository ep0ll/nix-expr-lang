use nix_expr_lang::{
    EvalContext, ErrorKind, HostError, LoaderHost, SourceUnit, Value,
};

fn eval_integer(source: &str) -> i64 {
    let mut context = EvalContext::new();
    match context.eval(source).expect("expression should evaluate") {
        Value::Integer(value) => value,
        other => panic!("expected integer, got {other:?}"),
    }
}

#[test]
fn arithmetic_precedence_and_integer_division() {
    assert_eq!(eval_integer("1 + 2 * 3"), 7);
    assert_eq!(eval_integer("9 / 2"), 4);
}

#[test]
fn curried_functions_and_lexical_closures() {
    assert_eq!(eval_integer("let add = x: y: x + y; in add 19 23"), 42);
}

#[test]
fn set_pattern_parameters_and_defaults() {
    assert_eq!(eval_integer("({ a ? 1, b }: a + b) { b = 41; }"), 42);
}

#[test]
fn let_bindings_are_lazy_and_recursive() {
    assert_eq!(eval_integer("let unused = 1 / 0; answer = 40 + 2; in answer"), 42);
}

#[test]
fn recursive_attrsets_can_reference_their_own_fields() {
    let mut context = EvalContext::new();
    let value = context.eval("(rec { x = 20; y = x + 22; }).y").unwrap();
    assert!(matches!(value, Value::Integer(42)));
}

#[test]
fn lexical_bindings_shadow_with_scopes() {
    assert_eq!(eval_integer("let x = 1; in with { x = 2; }; x"), 1);
}

#[test]
fn boolean_operators_short_circuit() {
    let mut context = EvalContext::new();
    assert!(matches!(context.eval("false && (1 / 0 == 1)").unwrap(), Value::Bool(false)));
    assert!(matches!(context.eval("true -> true").unwrap(), Value::Bool(true)));
    assert!(matches!(context.eval("false -> (1 / 0 == 1)").unwrap(), Value::Bool(true)));
}

#[test]
fn selection_default_is_used_for_missing_attribute() {
    assert_eq!(eval_integer("{ present = 1; }.missing or 42"), 42);
}

#[test]
fn nested_attrpaths_and_inherit_work() {
    assert_eq!(eval_integer("let x = 42; in { inherit x; }.x"), 42);
    assert_eq!(eval_integer("{ a.b = 42; }.a.b"), 42);
}

#[test]
fn native_callback_can_leave_argument_lazy() {
    let mut context = EvalContext::new();
    context.register_native("ignore", 1, |_context, _arguments| Ok(Value::Integer(42)));
    let result = context.eval("ignore (1 / 0)").unwrap();
    assert!(matches!(result, Value::Integer(42)));
}

#[test]
fn host_can_construct_and_force_compound_values() {
    let mut context = EvalContext::new();
    let record = context.make_attrset([
        ("answer", Value::Integer(42)),
        ("label", Value::String("host".to_owned())),
    ]).unwrap();
    context.register_global("record", record);
    let value = context.eval("record.answer").unwrap();
    assert!(matches!(value, Value::Integer(42)));

    let list = context.make_list([Value::Integer(10), Value::Integer(20)]);
    assert!(matches!(context.force_list_item(&list, 1).unwrap(), Some(Value::Integer(20))));
}

#[test]
fn import_is_delegated_to_the_host() {
    let host = LoaderHost::new(Box::new(|path, _importer| {
        if path.raw == "./answer.nix" {
            Ok(SourceUnit::new("virtual:/answer.nix", "40 + 2"))
        } else {
            Err(HostError::new("unknown virtual source"))
        }
    }));
    let mut context = EvalContext::with_host(host);
    context.install_import();
    assert!(matches!(context.eval("import ./answer.nix").unwrap(), Value::Integer(42)));
}

#[test]
fn black_hole_detection_reports_infinite_recursion() {
    let mut context = EvalContext::new();
    let error = context.eval("let x = x; in x").unwrap_err();
    assert_eq!(error.kind, ErrorKind::InfiniteRecursion);
}
