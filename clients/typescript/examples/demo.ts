/**
 * Run a nescioDB server first:
 *
 *   nescio init /tmp/demodb --template real-estate
 *   nescio serve /tmp/demodb --port 7777
 *
 * then: npm run example   (or: node examples/demo.ts on Node 22+)
 */
import { NescioClient, claim } from "../src/index.ts";

const db = new NescioClient(process.argv[2] ?? "http://localhost:7777");

async function main() {
  console.log("health:", await db.health());

  await db.ingest("villa_1", claim.interval("price", 900_000, 1_000_000), "broker", {
    at: "2026-06-25",
  });

  const b = await db.bound("villa_1", "price", { at: "2026-07-03" });
  console.log(
    `villa_1.price: ${b.entropyBits.toFixed(2)} of ${b.maxEntropyBits.toFixed(2)} bits ` +
      `(knowledge ${(b.knowledgeRatio * 100).toFixed(0)}%)`,
  );
  if (b.region.kind === "intervals") console.log("  region:", b.region.intervals);

  // Erosion: the same query a year later, without new evidence.
  const later = await db.bound("villa_1", "price", { at: "2027-07-03" });
  console.log(`one year on: ${later.entropyBits.toFixed(2)} bits (the region widened on its own)`);

  console.log("certainly > 500k:", await db.certainly("villa_1", "price", { op: "gt", value: 500_000 }, { at: "2026-07-03" }));

  const world = await db.sample("villa_1", { seed: 7, at: "2026-07-03" });
  console.log("one sampled world:", world);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
