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

  // Entity handles for entity-centric code.
  const villa = db.entity("villa_1");
  console.log("certainly > 500k:", await villa.certainly("price", { op: "gt", value: 500_000 }, { at: "2026-07-03" }));

  const world = await villa.sample({ seed: 7, at: new Date("2026-07-03") });
  console.log("one sampled world:", world);

  // Value of Information: which evidence most improves the decision?
  const plan = await villa.decide({
    slot: "price",
    objective: { kind: "absolute_error" },
    target: 10_000,
    at: "2026-07-03",
    actions: [{
      name: "pull the land registry", slot: "price", cost: 40,
      source: { name: "land_registry", reliability: 1.0, axiomatic: true },
      answerWidth: 20_000,
    }],
  });
  console.log(`decide: ±${plan.startRisk.toFixed(0)} now -> ±${plan.validatedRisk?.toFixed(0)} after (${plan.units})`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
