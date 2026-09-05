# Security

This program talks to a seedbox over mTLS and holds library paths and grabber config. Treat `config.toml` and `tls/` as secrets. Bootstrap refuses to mint PEMs inside a git work tree.

## Reporting

Please do **not** open a public issue for a vulnerability.

- Prefer a [GitHub security advisory](https://github.com/cotocisternas/mediaops/security/advisories/new)
- Or email [coto@petabyte.cl](mailto:coto@petabyte.cl)

Include the version (`mediaops --version` or the git SHA), what you ran, and what happened. We will acknowledge the report and fix before any public write-up.
