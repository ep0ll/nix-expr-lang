use nix_expr_lang::{
    EvaluatorBuilder,
    NixValueOps,
    Result,
    primop::PrimOpRet,
};

fn main() -> Result<()> {
    let mut builder = EvaluatorBuilder::new()?;

    // This is an application capability exposed as a real Nix primop. The implementation
    // can call an application's HTTP client, checksum verifier, artifact cache and store API.
    builder.register_primop(
        "host_fetch_metadata",
        1,
        Some("Fetch package metadata through the embedding application"),
        |arguments, result: &mut PrimOpRet<'_>| {
            let url = arguments[0].as_string()?;

            // Replace this stand-in with application-specific fetching and verification.
            result.set_string(&format!("resolved by host: {url}"))
        },
    )?;

    let evaluator = builder.build()?;
    let value = evaluator.eval(r#"
        {
          source = builtins.host_fetch_metadata "https://example.invalid/source";
          answer = 40 + 2;
        }
    "#)?;

    let answer = value.get_attr("answer")?;
    assert_eq!(answer.as_int()?, 42);

    let source = value.get_attr("source")?;
    println!("{}", source.as_string()?);

    Ok(())
}
