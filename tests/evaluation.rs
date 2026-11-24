use serial_test::serial;

use nix_expr_lang::{
    Evaluator,
    EvaluatorBuilder,
    NixValueOps,
    Result,
    ValueType,
    primop::PrimOpRet,
};

#[test]
#[serial]
fn delegates_expression_evaluation_to_native_libexpr() -> Result<()> {
    let evaluator = Evaluator::new()?;

    let value = evaluator.eval(r#"
        let
          add = x: y: x + y;
          unused = throw "call-by-need: this binding is never forced";
          config = rec {
            base = 19;
            answer = add base 23;
          };
        in
          if config.answer == 42 then config else throw "unexpected result"
    "#)?;

    assert_eq!(value.value_type(), ValueType::Attrs);
    assert_eq!(value.get_attr("answer")?.as_int()?, 42);
    Ok(())
}

#[test]
#[serial]
fn preserves_lazy_attribute_values_and_lists() -> Result<()> {
    let evaluator = Evaluator::new()?;
    let value = evaluator.eval(r#"
        {
          answer = 42;
          notNeeded = throw "this field must remain lazy";
          values = [ 1 (throw "this element must remain lazy") 3 ];
        }
    "#)?;

    let unused = value.get_attr_lazy("notNeeded")?;
    assert_eq!(unused.value_type(), ValueType::Thunk);

    assert_eq!(value.get_attr("answer")?.as_int()?, 42);

    let values = value.get_attr("values")?;
    values.force()?;
    assert_eq!(values.list_len()?, 3);

    let middle = values.list_get_lazy(1)?;
    assert_eq!(middle.value_type(), ValueType::Thunk);
    assert_eq!(values.list_get(0)?.as_int()?, 1);
    assert_eq!(values.list_get(2)?.as_int()?, 3);
    Ok(())
}

#[test]
#[serial]
fn native_primop_is_called_by_nix() -> Result<()> {
    let mut builder = EvaluatorBuilder::new()?;
    builder.register_primop(
        "nix_expr_lang_test_double_2026",
        1,
        Some("Test-only native primop"),
        |args, result: &mut PrimOpRet<'_>| {
            result.set_int(args[0].as_int()? * 2)
        },
    )?;

    let evaluator = builder.build()?;
    let value = evaluator.eval("builtins.nix_expr_lang_test_double_2026 21")?;
    assert_eq!(value.as_int()?, 42);
    Ok(())
}

#[test]
#[serial]
fn native_file_evaluator_resolves_relative_imports() -> Result<()> {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    std::fs::write(directory.path().join("answer.nix"), "40 + 2").expect("write answer.nix");
    std::fs::write(
        directory.path().join("default.nix"),
        "import ./answer.nix",
    ).expect("write default.nix");

    let evaluator = Evaluator::new()?;
    let value = evaluator.eval_file(directory.path().join("default.nix"))?;
    assert_eq!(value.as_int()?, 42);
    Ok(())
}
