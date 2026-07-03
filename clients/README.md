# nescioDB clients

Language clients for the nescioDB HTTP API. Start a server, then talk to it
from anywhere:

```bash
nescio init mydb --template real-estate
nescio serve mydb --port 7777
```

| Client | Path | Runtime deps | Verbs |
|---|---|---|---|
| Python | [`python/`](python/) | none (`urllib`, Python 3.9+) | all, incl. `decide` + schema evolution |
| TypeScript / JavaScript | [`typescript/`](typescript/) | none (global `fetch`, Node 18+) | all, incl. `decide` + schema evolution |
| Java | [`java/`](java/) | none (JDK 17+ `java.net.http` + bundled JSON codec) | `bound` … `resolve`, ingest, sources, priors |

All three are deliberately **single-file vendorable**: if you'd rather not add
a package, copy the one source file into your project and you are done.

Every client returns typed results — including the graded probability *and*
three-valued certainty that joins carry — and raises/throws a typed error with
the HTTP status and the server's message. See each directory's README for
install and usage.
