# nescioDB Java client

Zero-dependency client for the [nescioDB](../../) HTTP API. No Jackson, no Gson —
HTTP via the JDK's `java.net.http` and a small bundled JSON codec. Drop the
three files in `src/main/java/dev/nescio/` into your project and compile, or
build the jar with Maven.

Requires JDK 17+.

```java
import dev.nescio.NescioClient;
import dev.nescio.NescioClient.*;

var db = new NescioClient("http://localhost:7777");

db.ingest("villa_1", Claim.interval("price", 900_000, 1_000_000), "broker",
          At.date("2026-06-25"));

Bound b = db.bound("villa_1", "price", At.date("2026-07-03"));
System.out.printf("%.2f bits, knowledge %.0f%%%n",
    b.entropyBits(), b.knowledgeRatio() * 100);

// The same query a year later — the region has widened on its own.
db.bound("villa_1", "price", At.date("2027-07-03"));

// Joins over uncertain regions carry a probability AND a certainty.
JoinResult r = db.join(
    JoinPredicate.approx("price", "price", 50_000),
    JoinOptions.defaults().minProbability(0.5), null);
for (JoinMatch m : r.matches())
    System.out.println(m.left() + " " + m.right() + " " + m.probability() + " " + m.certainty());
```

## API

Methods mirror the server verbs and return Java records:

- `bound`, `sample`, `certainly`, `find`, `join`, `resolve`
- `ingest`, `ingestBatch`, `putSource`, `forgetSource`, `recalibrate`
- `registerPrior`, `usePrior`, `health`, `status`

Times (`At`) are `At.date("2026-07-03")` or `At.unix(seconds)`; pass `null` for
now. Non-2xx responses throw `NescioException` with `.status()` and the server
message.

## Build

```bash
# Plain javac — no build tool, no dependencies:
javac -d out src/main/java/dev/nescio/*.java example/Demo.java
java -cp out:example Demo http://localhost:7777

# Or Maven:
mvn package
```
