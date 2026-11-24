use nix_expr_lang::{
    Error, ErrorKind, EvalContext, NixPath, NixPathKind, Result, Value,
};

fn main() -> Result<()> {
    let mut evaluator = EvalContext::new();

    // This is application code, not an interpreter builtin. The real implementation can
    // use reqwest, a content-addressed cache, a credential provider, and a Nix store client.
    evaluator.register_native("fetchurl", 1, |context, arguments| {
        let specification = context.force(arguments[0])?;
        let url = context.force_attr(&specification, "url")?
            .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "fetchurl requires url"))?;

        let Value::String(url) = url else {
            return Err(Error::new(ErrorKind::Type, "fetchurl url must be a string"));
        };

        // Stand-in for the store path returned by an actual host fetcher.
        // The callback controls network and filesystem capabilities.
        let store_path = format!("/nix/store/example-source-{}", url.len());
        Ok(Value::Path(NixPath::new(store_path, NixPathKind::Absolute)))
    });

    let result = evaluator.eval(r#"
        let
          source = fetchurl {
            url = "https://example.invalid/archive.tar.gz";
            hash = "sha256-application-verifies-this";
          };
        in
          { src = source; unpack = true; }
    "#)?;

    let source = evaluator.force_attr(&result, "src")?
        .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "result has no src field"))?;
    println!("{source:?}");
    Ok(())
}
