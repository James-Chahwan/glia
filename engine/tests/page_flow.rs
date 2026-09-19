//! LA.6e — `pages::page_flow`: the answer over LA.6a-6c's page flow.
//!
//! A dead deep link is an unresolved `NAVIGATES_TO` ref, not an edge, so no
//! key.json field can assert it (bench/substrate-gap/fixtures/nav-angular-links
//! says so in its note). This file is where the quokka `/connect` defect is
//! pinned: the Angular app below mirrors that fixture file for file.

use std::path::Path;

use repo_graph_engine::generate_one;
use repo_graph_engine::pages::{PageFlow, page_flow};

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

/// bench/substrate-gap/fixtures/nav-angular-links, verbatim: a route table
/// with a `**` -> /login catch-all and no `connect`, a share link built from
/// `window.location.origin`, template links and `router.navigate` forms.
fn angular_app(root: &Path) {
    write(
        root,
        "src/app/app.routes.ts",
        "import { Routes } from '@angular/router';\n\
         import { HomeComponent } from './home/home.component';\n\
         import { LoginComponent } from './login.component';\n\
         import { ProfileComponent } from './profile.component';\n\
         import { UserViewComponent } from './user-view.component';\n\
         \n\
         export const routes: Routes = [\n\
         \x20 { path: 'home', component: HomeComponent },\n\
         \x20 { path: 'login', component: LoginComponent },\n\
         \x20 { path: 'verify-email', component: LoginComponent },\n\
         \x20 { path: 'profile', component: ProfileComponent },\n\
         \x20 { path: 'user/:publicId', component: UserViewComponent },\n\
         \x20 { path: '**', redirectTo: '/login' },\n\
         ];\n",
    );
    write(
        root,
        "src/app/home/home.component.html",
        "<a routerLink=\"/home\">Home</a>\n\
         <div *ngFor=\"let m of members\">\n\
         \x20 <a [routerLink]=\"m.id ? ['/user', m.id] : null\">user</a>\n\
         </div>\n\
         <img src=\"/logo.png\">\n",
    );
    write(
        root,
        "src/app/home/home.component.ts",
        "import { Component } from '@angular/core';\n\
         \n\
         @Component({ selector: 'app-home', templateUrl: './home.component.html' })\n\
         export class HomeComponent {\n\
         \x20 members: { id: string }[] = [];\n\
         }\n",
    );
    write(
        root,
        "src/app/login.component.ts",
        "import { Component } from '@angular/core';\n\
         import { Router } from '@angular/router';\n\
         \n\
         @Component({ selector: 'app-login', template: '<p>login</p>' })\n\
         export class LoginComponent {\n\
         \x20 constructor(private router: Router) {}\n\
         \x20 ok() {\n\
         \x20   this.router.navigate(['/home']);\n\
         \x20 }\n\
         \x20 verify() {\n\
         \x20   this.router.navigateByUrl('/verify-email');\n\
         \x20 }\n\
         \x20 back(p: string) {\n\
         \x20   this.router.navigate([p]);\n\
         \x20 }\n\
         }\n",
    );
    write(
        root,
        "src/app/profile.component.ts",
        "import { Component } from '@angular/core';\n\
         import { Router } from '@angular/router';\n\
         \n\
         @Component({ selector: 'app-profile', template: '<p>profile</p>' })\n\
         export class ProfileComponent {\n\
         \x20 id = 'abc';\n\
         \x20 constructor(private router: Router) {}\n\
         \x20 get shareLink(): string {\n\
         \x20   return `${window.location.origin}/connect?ref=${this.id}`;\n\
         \x20 }\n\
         }\n",
    );
    write(
        root,
        "src/app/user-view.component.ts",
        "import { Component } from '@angular/core';\n\
         \n\
         @Component({ selector: 'app-user', template: '<p>user</p>' })\n\
         export class UserViewComponent {}\n",
    );
}

fn build(root: &Path) -> PageFlow {
    let merged = generate_one(&root.to_string_lossy())
        .expect("generate_one")
        .merged;
    page_flow(&merged)
}

#[test]
fn angular_share_link_is_dead_and_absorbed_by_the_catch_all() {
    let dir = tempfile::tempdir().expect("tempdir");
    angular_app(dir.path());
    let flow = build(dir.path());

    assert_eq!(flow.dead.len(), 1, "exactly the /connect share link: {flow:#?}");
    let dead = &flow.dead[0];
    assert_eq!(dead.link, "/connect");
    assert!(
        dead.from_qname.ends_with("ProfileComponent"),
        "lifted onto the page component: {dead:?}"
    );
    assert_eq!(dead.from_file.as_deref(), Some("src/app/profile.component.ts"));
    assert!(dead.from_line.is_some_and(|l| l >= 1), "1-based: {dead:?}");
    assert_eq!(dead.absorbed_by.as_deref(), Some("/** -> /login"));

    let paths: Vec<&str> = flow.pages.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(
        paths,
        ["/**", "/home", "/login", "/profile", "/user/:publicId", "/verify-email"],
        "every nav route is a page, sorted by path"
    );
    let home = &flow.pages[1];
    assert!(
        home.handler.as_deref().is_some_and(|h| h.ends_with("HomeComponent")),
        "{home:?}"
    );
    assert_eq!(home.handler_file.as_deref(), Some("src/app/home/home.component.ts"));
    assert!(home.handler_line.is_some(), "{home:?}");
    assert_eq!(home.inbound_links, 2, "the template and LoginComponent.ok");
    assert!(!home.catchall && home.redirect_to.is_none());

    let catch_all = &flow.pages[0];
    assert!(catch_all.catchall, "{catch_all:?}");
    assert_eq!(catch_all.redirect_to.as_deref(), Some("/login"));
    assert_eq!(catch_all.handler, None);
    assert_eq!(flow.pages[2].inbound_links, 1, "/login: the catch-all's redirect");

    // Links are page -> page navigations; the redirect is on its page.
    let links: Vec<(String, &str, &str)> = flow
        .links
        .iter()
        .map(|l| {
            let from = l.from_qname.rsplit("::").next().unwrap_or("").to_string();
            (from, l.to_path.as_str(), l.confidence)
        })
        .collect();
    assert_eq!(
        links,
        [
            ("HomeComponent".to_string(), "/home", "strong"),
            ("HomeComponent".to_string(), "/user/:publicId", "medium"),
            ("LoginComponent".to_string(), "/home", "strong"),
            ("LoginComponent".to_string(), "/verify-email", "strong"),
        ]
    );
    assert!(flow.links.iter().all(|l| l.from_file.is_some()));

    // /login is reached only by the redirect, which counts; the catch-all is
    // never a page anyone links to, so it is not listed.
    assert_eq!(flow.unlinked, vec!["/profile".to_string()]);

    let json = serde_json::to_value(&flow).expect("serialises");
    let keys: Vec<&str> = json
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 4, "{json}");
    for key in ["pages", "links", "dead", "unlinked"] {
        assert!(keys.contains(&key), "missing `{key}`: {json}");
    }
}

#[test]
fn a_link_a_backend_route_serves_is_not_dead() {
    // LA.6a's `server` exemption sees only same-graph ROUTEs; a Flask route
    // sits in the Python graph, so the graph keeps the ref and page_flow drops
    // it. Control: the same app's link to a path nothing serves stays dead.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "web/src/app/app.routes.ts",
        "import { Routes } from '@angular/router';\n\
         import { HomeComponent } from './home.component';\n\
         \n\
         export const routes: Routes = [\n\
         \x20 { path: 'home', component: HomeComponent },\n\
         ];\n",
    );
    write(
        root,
        "web/src/app/home.component.ts",
        "import { Component } from '@angular/core';\n\
         import { Router } from '@angular/router';\n\
         \n\
         @Component({ selector: 'app-home', template: '<p>home</p>' })\n\
         export class HomeComponent {\n\
         \x20 constructor(private router: Router) {}\n\
         \x20 signIn() {\n\
         \x20   this.router.navigateByUrl('/auth/start');\n\
         \x20 }\n\
         \x20 lost() {\n\
         \x20   this.router.navigateByUrl('/nowhere');\n\
         \x20 }\n\
         }\n",
    );
    write(
        root,
        "api/app.py",
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
         @app.route('/auth/start')\ndef auth_start():\n    return {}\n",
    );
    let flow = build(root);
    let dead: Vec<&str> = flow.dead.iter().map(|d| d.link.as_str()).collect();
    assert_eq!(dead, ["/nowhere"], "{flow:#?}");
    assert_eq!(flow.dead[0].absorbed_by, None, "no catch-all in this app");
    assert_eq!(
        flow.pages.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(),
        ["/home"],
        "the Flask route is a server route, never a page"
    );
    assert_eq!(flow.unlinked, vec!["/home".to_string()]);
}

#[test]
fn page_flow_is_deterministic() {
    let dir = tempfile::tempdir().expect("tempdir");
    angular_app(dir.path());
    let merged = generate_one(&dir.path().to_string_lossy())
        .expect("generate_one")
        .merged;
    let a = serde_json::to_string(&page_flow(&merged)).expect("serialises");
    let b = serde_json::to_string(&page_flow(&merged)).expect("serialises");
    assert_eq!(a, b);
}
