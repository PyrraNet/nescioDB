# nescioDB clients

Language clients for the nescioDB HTTP API. Start a server, then talk to it
from anywhere:

```bash
nescio init mydb --template real-estate
nescio serve mydb --port 7777
```

| Client | Path | Runtime deps |
|---|---|---|
| TypeScript / JavaScript | [`typescript/`](typescript/) | none (uses global `fetch`) |
| Java | [`java/`](java/) | none (JDK 11+ `java.net.http` + a bundled JSON codec) |

Both wrap every verb — `bound`, `sample`, `certainly`, `find`, `join`,
`resolve` — plus ingest, sources, priors, calibration and GDPR erasure. Each
returns typed results, including the graded probability *and* three-valued
certainty that joins carry. See each directory's README for install and usage.
