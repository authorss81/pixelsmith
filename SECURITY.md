# Security Policy

See **[docs/SECURITY.md](docs/SECURITY.md)** for the full policy.

To report a vulnerability, use **Security → Report a vulnerability** on this
repository. Please do not open a public issue.

Short version: this app's entire job is opening files the user did not create, so
the image decoder is the attack surface. Security here means no panic on
untrusted input, bounded memory before allocation, contained failure, no
capability to exfiltrate, and no privacy leakage by default. The no-network
claim is verifiable from the source and the shipped binary — see the full policy
for the exact commands.