# OIDC cryptographic fixtures

`oidc-private.pem` and `oidc-jwks.json` are a disposable RSA key pair generated exclusively for automated tests. They are public test fixtures, not deployment credentials. Integration tests trust the public JWK only in their isolated database. Production always fetches GitHub's official signing keys and never reads these files.
