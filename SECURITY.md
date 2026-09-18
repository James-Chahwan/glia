# Security

glia builds a static map of source code: the services in a codebase and the routes, calls, queues, data
stores, config and infrastructure that connect them. It maps structure, not exploitability. This page
covers how to report a vulnerability in glia itself and which capabilities glia deliberately does not
build.

## Reporting a vulnerability in glia

Please report it privately. Use GitHub's private vulnerability reporting on
[James-Chahwan/glia](https://github.com/James-Chahwan/glia/security/advisories/new) (**Security →
Report a vulnerability**). Do not open a public issue for it. If private reporting is unavailable, open
an issue that asks for a private contact and leave the details out.

In scope: defects in glia's own code. For example:

- a crafted source file, manifest or `.gmap` that crashes the parser or loader, or makes it hang;
- a build that reads or writes outside the repo and output directory it was given;
- a secret value that gets past redaction and is stored in a graph (see below).

Please include:

- the glia version (`glia --version`) or commit, and your platform;
- the smallest input that reproduces the problem (a file, a repo layout or a `.gmap`);
- what happened, and what you expected to happen.

There is no bug bounty.

## Run glia only on code you are authorised to analyse

glia maps structure for the people who own the code. A team can see its own services, find dead routes
and blind spots, and know what a change touches before it ships. Run it only on code you own or have
permission to analyse.

## What glia does not build

glia does not ship the following in core, and will not accept them as contributions:

- **Taint or value data-flow analysis.** Tracing which input reaches which sink across services turns a
  structural map into a list of exploitation targets.
- **Joining package or call reachability to vulnerability (CVE) feeds.** A list of the services that
  reach a known-vulnerable function is a ready-made attack graph. glia emits the dependency substrate
  (`PACKAGE_DEP`, `PackageResolver`) and stops there.
- **Mass-corpus crawling or bulk-scan modes.** glia analyses only the repos you point it at. It does not
  fetch, crawl or batch-scan code it was not given (`glia merge` takes repos already on disk).
- **Lists of routes that reach data without authentication.** Whatever the intent of the person
  running it, an inventory of unauthenticated routes that reach data is an attack plan.

## Untrusted text is redacted before it is stored

Config values that glia reads (Dockerfile `ENV`, `.env` files, k8s and compose environments) are
redacted before they reach a graph. A secret-named key records only that a value exists, credentials
inside a URL are masked, and long values are capped. Any other untrusted text glia ingests, such as CI
logs or test reports, goes through the same redaction before it is stored.

## How security-relevant changes are reviewed

Every new kind of edge, cell or answer is checked against the list above before it lands. The question is
whether it moves glia from mapping structure toward generating exploitation targets. If it does, it does
not land in core. Changes to untrusted-input parsing, the `.gmap` loader or redaction get the same
explicit review.
