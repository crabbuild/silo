# Security policy

SILO is an open-source library, not a hosted public service. Do not disclose
credentials, provider endpoints, repository prefixes, or repository data in
bug reports.

## Reporting a vulnerability

Do not open a public issue. Use GitHub's private vulnerability reporting for
`crabuild/silo`, or contact the repository owners through the CrabBuild
security channel. Include:

- the affected commit or release;
- a minimal reproduction without customer data;
- impact and exploitability;
- any known mitigations.

We will acknowledge receipt, coordinate a fix, and decide on disclosure after
affected deployments have a remediation path.

## Operational requirements

- Never commit AWS, RustFS, HMAC, or attestation credentials.
- Keep the SILO repository prefix exclusive to the client.
- Use isolated versioned buckets for qualification tests.
- Treat provider request IDs and repository identifiers as sensitive metadata.
