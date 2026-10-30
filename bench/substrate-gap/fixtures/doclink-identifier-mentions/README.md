# Registry

## Usage

The `ServiceRegistry` keeps every service, retrying `MAX_RETRIES` times.
Run `run` to start, inspect the `graph`, dump `json`, and look for `CALLS` edges.
Tests build one with `make_registry`.

## Lookup

Call `ServiceRegistry.lookup` or `lookup_service()` to find one.

## Deploy

```sh
gcloud run deploy --image=us-docker.pkg.dev/PROJECT_ID/registry
```

Where `PROJECT_ID` is your Google Cloud project id.
