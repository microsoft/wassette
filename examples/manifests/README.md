# Provisioning manifests

A provisioning manifest defines the components that Wassette loads at startup.
Pass the YAML file to `wassette serve` with `--manifest`, for example:

```bash
wassette serve --manifest examples/manifests/simple.yaml
```

## Format

The top level is a mapping with two required fields. `version` is the manifest
schema version, and its only supported value is the integer `1`. `components`
is a non-empty sequence of component declarations. Each component must have a
unique `uri`.

Every component declaration requires `uri` and `permissions`. The `uri` locates
the component with an `oci://`, `file://`, `https://`, or `http://` URI.
`permissions` is required even when the component needs no inline permission
overrides, in which case use `permissions: {}`. The optional `name` identifies
the component in logs. The optional `digest` is an expected SHA-256 digest in
the form `sha256:` followed by 64 hexadecimal characters. The optional
`retry_policy` accepts an attempt count and either an `exponential` backoff with
`base_ms` or a `linear` backoff with `increment_ms`; retry handling is currently
deferred.

The required `permissions` mapping can contain optional `network`, `storage`,
`environment`, and `resources` permission types. A network permission has a
non-empty `allow` sequence of `host` values. A storage permission has a
non-empty `allow` sequence whose entries pair an `fs://` `uri` with one or both
of the `read` and `write` access types. An environment permission has a
non-empty `allow` sequence of variable `key` values. Each environment entry can
also set `value_from` to read the value from a differently named variable in
the Wassette process environment. If `value_from` is omitted, Wassette reads
the variable named by `key`. The optional `resources` permission accepts
`memory_bytes` and `cpu_time_ms`, but resource limit enforcement is currently
deferred.

## Examples

Start with [`simple.yaml`](simple.yaml), which shows the minimum practical
manifest for a component that needs network access and an environment secret.
[`multi-component.yaml`](multi-component.yaml) adds components with empty and
storage permissions. [`with-secrets.yaml`](with-secrets.yaml) demonstrates
explicit `value_from` mappings. [`production.yaml`](production.yaml)
illustrates version and digest pinning with placeholder values that must be
replaced before use.
