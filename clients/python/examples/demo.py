"""Run a nescioDB server first:

    nescio init /tmp/demodb --template real-estate
    nescio serve /tmp/demodb --port 7777

then: python3 examples/demo.py [base_url]
"""

import sys
from datetime import date

sys.path.insert(0, "src")  # run from the repo without installing
from nesciodb import NescioClient, action, claim, source

db = NescioClient(sys.argv[1] if len(sys.argv) > 1 else "http://localhost:7777")

print("health:", db.health())

db.ingest("villa_1", claim.interval("price", 900_000, 1_000_000), "broker", at="2026-06-25")

b = db.bound("villa_1", "price", at=date(2026, 7, 3))
print(f"villa_1.price: {b.entropy_bits:.2f} of {b.max_entropy_bits:.2f} bits "
      f"(knowledge {b.knowledge_ratio:.0%})")
print("  region:", b.region)

# Erosion: the same query a year later, without new evidence.
later = db.bound("villa_1", "price", at=date(2027, 7, 3))
print(f"one year on: {later.entropy_bits:.2f} bits (the region widened on its own)")

villa = db.entity("villa_1")
print("certainly > 500k:", villa.certainly("price", gt=500_000, at=date(2026, 7, 3)))

world = villa.sample(seed=7, at=date(2026, 7, 3))
print("one sampled world:", world)

# Ask the DB what evidence would help most.
plan = villa.resolve(
    "price",
    target_bits=2.0,
    at=date(2026, 7, 3),
    actions=[
        action("call the broker", "price", cost=5,
               src=source("broker", 0.85, half_life_days=90), answer_width=100_000),
        action("pull the land registry", "price", cost=40,
               src=source("land_registry", 1.0, axiomatic=True), answer_width=20_000),
    ],
)
for i, step in enumerate(plan.steps, 1):
    print(f"  {i}. {step.action['name']} -> {step.expected_entropy_bits:.2f} bits")
print(f"MC-validated: {plan.validated_entropy_bits:.2f} bits")
