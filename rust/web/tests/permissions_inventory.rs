//! SH-02 drift guard, not a substitute for HTTP/WS authorization tests.
use quote::ToTokens;
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path};
use syn::{
    visit::{self, Visit},
    Expr, ExprCall, ExprMethodCall, ItemEnum, ItemFn, ItemMod, Lit,
};

#[derive(Default)]
struct Inventory {
    http: BTreeMap<String, Vec<String>>,
    wire: BTreeMap<String, Vec<String>>,
    mounts: Vec<Vec<String>>,
    source: String,
    in_router: bool,
}
fn text(v: &impl ToTokens) -> String {
    v.to_token_stream().to_string()
}
fn literal(e: &Expr) -> Option<String> {
    if let Expr::Lit(v) = e {
        if let Lit::Str(s) = &v.lit {
            return Some(s.value());
        }
    }
    None
}
fn methods(e: &Expr) -> Vec<(String, String)> {
    match e {
        Expr::Call(c) => {
            let name = text(&c.func);
            assert!(
                matches!(
                    name.as_str(),
                    "get" | "post" | "delete" | "put" | "patch" | "head" | "options"
                ),
                "unreviewed route method: {name}"
            );
            assert_eq!(c.args.len(), 1);
            assert!(
                matches!(&c.args[0], Expr::Path(_)),
                "unreviewed route handler"
            );
            vec![(name.to_uppercase(), text(&c.args[0]))]
        }
        Expr::MethodCall(c) => {
            let mut out = methods(&c.receiver);
            let name = c.method.to_string();
            if name != "layer" {
                assert!(
                    matches!(
                        name.as_str(),
                        "get" | "post" | "delete" | "put" | "patch" | "head" | "options"
                    ),
                    "unreviewed method chain: {name}"
                );
                assert_eq!(c.args.len(), 1);
                assert!(
                    matches!(&c.args[0], Expr::Path(_)),
                    "unreviewed chained handler"
                );
                out.push((name.to_uppercase(), text(&c.args[0])));
            }
            out
        }
        _ => panic!("unreviewed dynamic method router: {}", text(e)),
    }
}
impl Inventory {
    fn paths(&self, e: &Expr) -> Vec<String> {
        if let Some(s) = literal(e) {
            return vec![s];
        }
        assert!(
            matches!(
                self.source.as_str(),
                "drc/review/http.rs" | "drc/review/transfer.rs"
            ),
            "unreviewed dynamic route in {}",
            self.source
        );
        let template = if text(e) == "root" {
            "{root}".to_string()
        } else {
            let Expr::Reference(r) = e else {
                panic!("unreviewed route expression")
            };
            let Expr::Macro(m) = r.expr.as_ref() else {
                panic!("unreviewed route expression")
            };
            assert!(m.mac.path.is_ident("format"));
            syn::parse2::<syn::LitStr>(m.mac.tokens.clone())
                .expect("literal-only route format")
                .value()
        };
        assert!(template.starts_with("{root}"));
        ["notes", "waives"]
            .iter()
            .map(|kind| {
                template
                    .replace("{root}", &format!("/api/v1/drc/review/{kind}"))
                    .replace("{{", "{")
                    .replace("}}", "}")
            })
            .collect()
    }
    fn parse(&mut self, source: &str, code: &str) {
        self.source = source.into();
        self.visit_file(&syn::parse_file(code).expect("valid Rust source"));
    }
    fn value(&self) -> Value {
        let mut mounts = self.mounts.clone();
        mounts.sort();
        json!({"http":self.http,"wire":self.wire,"mounts":mounts})
    }
}
impl<'ast> Visit<'ast> for Inventory {
    fn visit_item_fn(&mut self, f: &'ast ItemFn) {
        let previous = self.in_router;
        self.in_router = previous
            || text(&f.sig.output).contains("Router")
            || matches!(
                f.sig.ident.to_string().as_str(),
                "routes" | "router" | "routes_for"
            );
        visit::visit_item_fn(self, f);
        self.in_router = previous;
    }
    fn visit_expr_macro(&mut self, m: &'ast syn::ExprMacro) {
        if self.in_router {
            assert!(m.mac.path.is_ident("format"), "unreviewed routing macro");
        }
        visit::visit_expr_macro(self, m);
    }
    fn visit_item_mod(&mut self, m: &'ast ItemMod) {
        if !m.attrs.iter().any(|a| text(&a.meta) == "cfg (test)") {
            visit::visit_item_mod(self, m);
        }
    }
    fn visit_expr_method_call(&mut self, c: &'ast ExprMethodCall) {
        let name = c.method.to_string();
        if name == "route" {
            assert_eq!(c.args.len(), 2);
            let mut verbs = methods(&c.args[1]);
            if !verbs.iter().any(|(v, _)| v == "HEAD") {
                if let Some((_, handler)) = verbs.iter().find(|(v, _)| v == "GET") {
                    verbs.push(("HEAD".into(), handler.clone()));
                }
            }
            for path in self.paths(&c.args[0]) {
                for (method, handler) in &verbs {
                    assert!(
                        self.http
                            .insert(
                                format!("{method} {path}"),
                                vec![self.source.clone(), handler.clone()]
                            )
                            .is_none(),
                        "duplicate route {method} {path}"
                    );
                }
            }
        } else if matches!(name.as_str(), "merge" | "fallback") {
            self.mounts.push(vec![
                self.source.clone(),
                format!("{name}({})", text(&c.args)),
            ]);
        } else {
            assert!(
                !matches!(
                    name.as_str(),
                    "route_service"
                        | "nest"
                        | "nest_service"
                        | "fallback_service"
                        | "method_not_allowed_fallback"
                        | "route_layer"
                ),
                "unreviewed routing construct: {name}"
            );
        }
        visit::visit_expr_method_call(self, c);
    }
    fn visit_expr_call(&mut self, c: &'ast ExprCall) {
        if let Expr::Path(p) = c.func.as_ref() {
            assert!(
                p.path.segments.last().is_none_or(|s| !matches!(
                    s.ident.to_string().as_str(),
                    "route" | "route_service" | "nest" | "nest_service"
                )),
                "unreviewed UFCS routing"
            );
        }
        // Pin both roots of the only parameterized route factory.
        if text(&c.func) == "routes_for" {
            self.mounts.push(vec![self.source.clone(), text(c)]);
        }
        visit::visit_expr_call(self, c);
    }
    fn visit_item_enum(&mut self, e: &'ast ItemEnum) {
        if e.ident != "Control" && !(self.source == "drc/dto.rs" && e.ident == "Request") {
            return;
        }
        let snake = e
            .attrs
            .iter()
            .any(|a| text(&a.meta).contains("rename_all = \"snake_case\""));
        for v in &e.variants {
            let mut renamed = None;
            for attr in &v.attrs {
                if attr.path().is_ident("serde") {
                    attr.parse_nested_meta(|m| {
                        assert!(m.path.is_ident("rename"), "unreviewed control attribute");
                        renamed = Some(m.value()?.parse::<syn::LitStr>()?.value());
                        Ok(())
                    })
                    .unwrap();
                }
            }
            let variant = v.ident.to_string();
            let wire = renamed.unwrap_or_else(|| {
                assert!(snake, "explicit wire name required: {variant}");
                let mut s = String::new();
                for (i, c) in variant.chars().enumerate() {
                    if i > 0 && c.is_ascii_uppercase() {
                        s.push('_');
                    }
                    s.push(c.to_ascii_lowercase());
                }
                s
            });
            assert!(self
                .wire
                .insert(format!("{}/{}/{wire}", self.source, e.ident), vec![variant])
                .is_none());
        }
    }
}
fn source_tree(path: &Path, base: &Path, out: &mut Inventory) {
    let mut entries = fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            source_tree(&path, base, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.parse(
                path.strip_prefix(base).unwrap().to_str().unwrap(),
                &fs::read_to_string(&path).unwrap(),
            );
        }
    }
}
fn current() -> Inventory {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Inventory::default();
    source_tree(&base, &base, &mut out);
    out
}
fn check(actual: &Value, policy: &Value) {
    let mut expected = policy.clone();
    for family in ["http", "wire"] {
        for row in expected[family].as_object_mut().unwrap().values_mut() {
            let fields = row.as_array_mut().unwrap();
            let authority = fields.pop().unwrap();
            assert!(
                [
                    "owner",
                    "public_static",
                    "owner_bootstrap",
                    "guest_bootstrap",
                    "guest",
                    "guest_drc",
                    "guest_explore",
                    "guest_drc_conditional"
                ]
                .contains(&authority.as_str().unwrap()),
                "unclassified authority"
            );
        }
    }
    assert_eq!(
        actual, &expected,
        "SH-02 surface changed: review authority and native denials before updating policy"
    );
}
#[test]
fn every_route_method_and_wire_variant_has_an_explicit_authority() {
    check(
        &current().value(),
        &serde_json::from_str(include_str!("../../../tools/web_permissions.json")).unwrap(),
    );
}
#[test]
fn parser_handles_comments_strings_multiline_and_rejects_dynamic_methods() {
    let mut inv = Inventory::default();
    inv.parse(
        "example.rs",
        r#"fn routes(){ let text=".route(ignored)"; // .route(ignored)
        Router::new().route("/safe", get(read).post(write)); }
        #[cfg(test)] mod tests { fn test(){ Router::new().route("/test", get(test)); } }"#,
    );
    assert_eq!(inv.http.len(), 3);
    assert!(inv.http.contains_key("HEAD /safe"));
    assert!(std::panic::catch_unwind(|| Inventory::default().parse(
        "bad.rs",
        "fn f(){ Router::new().route(path, any(handler)); }"
    ))
    .is_err());
    assert!(std::panic::catch_unwind(|| Inventory::default().parse(
        "bad.rs",
        "fn f(){ Router::new().nest(\"/new\", external()); }"
    ))
    .is_err());
    assert!(std::panic::catch_unwind(
        || Inventory::default().parse("bad.rs", "fn routes()->Router { install_routes!() }")
    )
    .is_err());
    assert!(std::panic::catch_unwind(|| Inventory::default().parse(
        "bad.rs",
        "fn routes()->Router { Router::route(r, \"/new\", get(read)) }"
    ))
    .is_err());
}
#[test]
fn adding_or_rebinding_a_route_or_control_fails_the_reviewed_table() {
    let policy: Value =
        serde_json::from_str(include_str!("../../../tools/web_permissions.json")).unwrap();
    let base = current().value();
    check(&base, &policy);
    let mut added = current();
    added.parse(
        "new.rs",
        "fn routes()->Router { Router::new().route(\"/new\", post(read)) }",
    );
    assert!(std::panic::catch_unwind(|| check(&added.value(), &policy)).is_err());
    let mut added = current();
    added.parse(
        "sharing/stream.rs",
        "enum Control { #[serde(rename=\"new\")] New { seq: String } }",
    );
    assert!(std::panic::catch_unwind(|| check(&added.value(), &policy)).is_err());
    let mut added = current();
    added.parse(
        "drc/review/http.rs",
        "fn routes()->Router { routes_for(store::Kind::Notes, \"/different\") }",
    );
    assert!(std::panic::catch_unwind(|| check(&added.value(), &policy)).is_err());
    for (family, key, row) in [
        ("http", "POST /new", json!(["new.rs", "read"])),
        ("wire", "sharing/stream.rs/Control/new", json!(["New"])),
    ] {
        let mut changed = base.clone();
        changed[family][key] = row;
        assert!(std::panic::catch_unwind(|| check(&changed, &policy)).is_err());
    }
    let mut changed = base;
    changed["http"]["GET /api/v1/catalog"][1] = json!("unguarded");
    assert!(std::panic::catch_unwind(|| check(&changed, &policy)).is_err());
}
#[test]
#[ignore = "diagnostic output only; never writes or approves the policy"]
fn print_inventory_for_review() {
    println!("INVENTORY_JSON={}", current().value());
}
