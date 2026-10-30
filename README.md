# Vunly SDKs

Client libraries for the Vunly API, one per language: TypeScript, Python, Go, Java, .NET, Rust, PHP and Ruby. This repository holds only the SDKs, their tests and the shared conformance material they need.

Vunly is usage billing for AI, agents and APIs. Check a customer before the work runs, record the usage once, and show the balance. Each SDK covers the same calls: customers, credits, usage, entitlements, plans, customer tokens and webhook verification.

Each SDK is written for the Vunly sandbox API. No real payments are made.

## Status

| SDK | Status | Versions tested | Known limitations |
| --- | --- | --- | --- |
| [TypeScript](typescript/README.md) | Implemented | Node.js 22 and 24 | [2 listed](typescript/README.md#known-limitations) |
| [Python](python/README.md) | Implemented | Python 3.10 to 3.13 | [3 listed](python/README.md#known-limitations) |
| [Go](go/README.md) | Implemented | Go 1.26 and 1.27 | [2 listed](go/README.md#known-limitations) |
| [Java](java/README.md) | Implemented | Java 17, 21 and 25 | [4 listed](java/README.md#known-limitations) |
| [.NET](dotnet/README.md) | Implemented | .NET 8 and 10 | [4 listed](dotnet/README.md#known-limitations) |
| [Rust](rust/README.md) | Implemented | Rust 1.85 minimum; run on 1.85.1 and 1.99.1 | [4 listed](rust/README.md#known-limitations) |
| [PHP](php/README.md) | Implemented | PHP 8.2 to 8.5 | [4 listed](php/README.md#known-limitations) |
| [Ruby](ruby/README.md) | Implemented | Ruby 3.3 and 4.0 | [5 listed](ruby/README.md#known-limitations) |

**Implemented** means the SDK works, has tests and passes our own checks. **Beta** means the same, plus a public way to get it (this repository), a documented install and a test you can run from this repository alone. None of these libraries is published to a package registry. Install each one from this repository, as its own README describes.

## Install and test

Every SDK has its own README with the install steps for that language and the command that runs its tests. The conformance suite that does not need the Vunly API runs in this repository's CI. Nothing here needs a secret.

## Run the tests

Each command runs from a clone of this repository and needs no Vunly API and no key: the tests use fake transports or local servers and synthetic keys. The first run downloads the test tools of that language from its usual package index.

**TypeScript**

```sh
cd typescript
npm test
```

**Python**

```sh
cd python
python3 -m venv .venv
.venv/bin/pip install -e ".[test]"
.venv/bin/python -m pytest
```

**Go**

```sh
cd go
go test ./...
```

**Java**

```sh
cd java
mvn -B -ntp clean test
```

**.NET**

```sh
cd dotnet
cd tests/Vunly.Tests
dotnet test -f net10.0 -p:TargetFrameworks=net10.0
```

**Rust**

```sh
cd rust
cargo test --locked
```

**PHP**

```sh
cd php
composer update --no-interaction
vendor/bin/phpunit --no-progress
```

**Ruby**

```sh
cd ruby
ruby -Ilib -Itest -e 'Dir["test/*_test.rb"].sort.each { |f| require "./#{f}" }'
```

## Conformance

The Retry-After rule is shared by all eight SDKs, and each SDK's tests run the same vectors: [conformance/retry-after-vectors.json](conformance/retry-after-vectors.json). The command above is therefore also the conformance run you can do here, and this repository's CI checks that every SDK's copy of the vectors is identical to that file.

The behavioural scenarios in [conformance/scenarios](conformance/scenarios) describe, in language-neutral JSON, how every SDK must behave against the Vunly API. Running them needs the Vunly API and a fault-injecting test harness that are not part of this repository, so you cannot run them from here. They are included so you can read what is promised. See [conformance/README.md](conformance/README.md).

## Known limitations

The known limitations of each SDK are in its README, in a section named **Known limitations**. They are also in the table above. A limitation does not mean the SDK is withdrawn; read it before you use the SDK.

## Support

There is no support commitment or service level. To report a vulnerability, read [SECURITY.md](SECURITY.md).

## License

Apache-2.0. See [LICENSE](LICENSE) and each SDK's `NOTICE`.
