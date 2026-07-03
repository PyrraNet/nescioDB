import dev.nescio.NescioClient;
import dev.nescio.NescioClient.At;
import dev.nescio.NescioClient.Bound;
import dev.nescio.NescioClient.Claim;
import dev.nescio.NescioClient.JoinPredicate;
import dev.nescio.NescioClient.JoinResult;
import dev.nescio.NescioClient.Predicate;

/**
 * Run a nescioDB server first:
 *
 *   nescio init /tmp/demodb --template real-estate
 *   nescio serve /tmp/demodb --port 7777
 *
 * then, from clients/java:
 *
 *   javac -d out src/main/java/dev/nescio/*.java example/Demo.java
 *   java -cp out:example Demo
 */
public class Demo {
    public static void main(String[] args) {
        String url = args.length > 0 ? args[0] : "http://localhost:7777";
        var db = new NescioClient(url);
        System.out.println("health: " + db.health());

        db.ingest("villa_1", Claim.interval("price", 900_000, 1_000_000), "broker", At.date("2026-06-25"));

        Bound b = db.bound("villa_1", "price", At.date("2026-07-03"));
        System.out.printf(
            "villa_1.price: %.2f of %.2f bits (knowledge %.0f%%)%n",
            b.entropyBits(), b.maxEntropyBits(), b.knowledgeRatio() * 100);
        if (b.region().isNumeric()) {
            double[] iv = b.region().intervals().get(0);
            System.out.printf("  region starts at [%.0f, %.0f]%n", iv[0], iv[1]);
        }

        // Erosion: the same query a year later, without new evidence.
        Bound later = db.bound("villa_1", "price", At.date("2027-07-03"));
        System.out.printf("one year on: %.2f bits (the region widened on its own)%n", later.entropyBits());

        System.out.println("certainly > 500k: "
            + db.certainly("villa_1", "price", Predicate.gt(500_000), At.date("2026-07-03")));

        JoinResult cmp = db.join(JoinPredicate.approx("price", "price", 50_000));
        System.out.println("comparable pairs: " + cmp.matches().size()
            + " (" + cmp.pairsExamined() + " examined)");
    }
}
