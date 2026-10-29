//! FD.31: the PACKAGE_DEPs of the remaining manifests (Maven pom.xml, Gradle,
//! pubspec.yaml, mix.exs, .NET project files) join identity's classes on a
//! real two-repo build (`generate_many`), the identity_resolver.rs pattern:
//! a dependency both repos declare is one `package_name` class, and a
//! dependency on a package the other repo's manifest names is one
//! `manifest_name` class with that PROJECT.
//!
//! Baseline before FD.31 (HEAD 0df1bbbb): none of these files minted a
//! PACKAGE_DEP (pom.xml, the Gradle and .NET files were not even walked), so
//! neither build held a SAME_AS edge.

use std::path::Path;

use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_core::NodeId;
use glia_engine::{GenerateResult, generate_many};

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, text).expect("write");
}

/// Every SAME_AS edge as `(from qname, to qname, rule)`, sorted.
fn stars(r: &GenerateResult) -> Vec<(String, String, String)> {
    let qname = |id: NodeId| {
        r.merged
            .graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
            .unwrap_or_else(|| panic!("no qname for {id:?}"))
    };
    let mut out: Vec<(String, String, String)> = r
        .merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::SAME_AS)
        .map(|e| {
            let ev = Evidence::of(e).unwrap_or_else(|| panic!("no evidence: {e:?}"));
            (qname(e.from), qname(e.to), ev.rule.unwrap_or_default())
        })
        .collect();
    out.sort();
    out
}

fn t(from: &str, to: &str, rule: &str) -> (String, String, String) {
    (from.to_string(), to.to_string(), rule.to_string())
}

#[test]
fn remaining_manifests_form_identity_classes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = tmp.path().join("app");
    let kit = tmp.path().join("kit");
    // The consumer: one manifest per directory, so each is its own PROJECT.
    write(
        &app,
        "jvm/pom.xml",
        "<project><groupId>com.example</groupId><artifactId>app</artifactId>\
         <dependencies><dependency><groupId>com.acme</groupId>\
         <artifactId>core</artifactId><version>1.0</version></dependency>\
         </dependencies></project>\n",
    );
    write(
        &app,
        "mobile/pubspec.yaml",
        "name: app\ndependencies:\n  money_kit: ^1.0.0\n  flutter:\n    sdk: flutter\n",
    );
    write(
        &app,
        "ex/mix.exs",
        "defmodule App.MixProject do\n  use Mix.Project\n  def project, do: [app: :app, deps: deps()]\n  defp deps, do: [{:shop_core, \"~> 0.1\"}]\nend\n",
    );
    write(
        &app,
        "net/App.csproj",
        "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>\
         <PackageReference Include=\"Acme.Billing\" Version=\"2.0.0\" />\
         </ItemGroup></Project>\n",
    );
    // The provider: the packages the consumer names, by their manifests.
    write(
        &kit,
        "jvm/build.gradle.kts",
        "dependencies {\n    implementation(\"com.acme:core:1.0\")\n}\n",
    );
    write(&kit, "dart/pubspec.yaml", "name: money_kit\n");
    write(
        &kit,
        "ex/mix.exs",
        "defmodule ShopCore.MixProject do\n  use Mix.Project\n  def project, do: [app: :shop_core, deps: []]\nend\n",
    );
    write(
        &kit,
        "net/Acme.Billing.csproj",
        "<Project Sdk=\"Microsoft.NET.Sdk\" />\n",
    );

    let paths = [
        app.to_string_lossy().into_owned(),
        kit.to_string_lossy().into_owned(),
    ];
    let r = generate_many(&paths).expect("builds");
    let got = stars(&r);
    let want = vec![
        // package_name: both repos depend on com.acme:core (a pom and a
        // Gradle file), the kit's copy -> the app's (first graph).
        t(
            "package:maven:com.acme:core",
            "package:maven:com.acme:core",
            "package_name",
        ),
        // manifest_name: each PROJECT whose manifest names the package the
        // app depends on -> that dependency (the least qname of the class).
        t("project:dart", "package:pub:money_kit", "manifest_name"),
        t("project:ex", "package:hex:shop_core", "manifest_name"),
        t("project:net", "package:nuget:Acme.Billing", "manifest_name"),
    ];
    assert_eq!(got, want);
    // The SDK entry is no package of either repo.
    let deps: Vec<String> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.values().cloned())
        .filter(|q| q.starts_with("package:"))
        .collect();
    assert!(!deps.iter().any(|q| q == "package:pub:flutter"), "{deps:?}");
}
