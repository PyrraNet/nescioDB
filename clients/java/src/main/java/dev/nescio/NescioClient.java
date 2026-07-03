package dev.nescio;

import java.net.URI;
import java.net.URLEncoder;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;

/**
 * nescioDB — a zero-dependency Java client for the HTTP API.
 *
 * <p>nescioDB is a database whose primary object is ignorance: fields hold
 * regions with entropy, not values. Start a server with {@code nescio serve},
 * then talk to it from anywhere. This client wraps every verb; it needs no
 * external libraries — just drop this file in and compile with {@code javac}.
 *
 * <pre>{@code
 * var db = new NescioClient("http://localhost:7777");
 * db.ingest("villa_1", Claim.interval("price", 900_000, 1_000_000), "broker",
 *           At.date("2026-06-25"));
 * Bound b = db.bound("villa_1", "price", At.date("2026-07-03"));
 * System.out.printf("%.2f bits, knowledge %.0f%%%n",
 *     b.entropyBits(), b.knowledgeRatio() * 100);
 * }</pre>
 */
public final class NescioClient {

    private final String base;
    private final HttpClient http;
    private final Duration timeout;

    public NescioClient(String baseUrl) {
        this(baseUrl, Duration.ofSeconds(30));
    }

    public NescioClient(String baseUrl, Duration timeout) {
        this.base = baseUrl.replaceAll("/+$", "");
        this.timeout = timeout;
        // Force HTTP/1.1: the server speaks 1.1 only, and the default h2c
        // upgrade attempt over cleartext would hang.
        this.http = HttpClient.newBuilder()
            .version(HttpClient.Version.HTTP_1_1)
            .connectTimeout(timeout)
            .build();
    }

    // -------------------------------------------------------- introspection

    public Map<String, Object> health() {
        return asObject(get("/health", Map.of()));
    }

    public Map<String, Object> status() {
        return asObject(get("/status", Map.of()));
    }

    // ---------------------------------------------------------------- verbs

    /** BOUND: credible region + entropy — how ignorant is the DB about this slot? */
    public Bound bound(String entity, String slot, At at) {
        return bound(entity, slot, at, null);
    }

    public Bound bound(String entity, String slot, At at, Double credible) {
        var params = new LinkedHashMap<String, Object>();
        params.put("entity", entity);
        params.put("slot", slot);
        if (at != null) params.put("at", at.value());
        if (credible != null) params.put("credible", credible);
        return Bound.from(asObject(get("/bound", params)));
    }

    /** SAMPLE: one concrete, consistent world, deterministic under the seed. */
    public Map<String, Object> sample(String entity, long seed, At at) {
        var params = new LinkedHashMap<String, Object>();
        params.put("entity", entity);
        params.put("seed", seed);
        if (at != null) params.put("at", at.value());
        return asObject(get("/sample", params));
    }

    /** Three-valued predicate: TRUE / POSSIBLE / FALSE (region containment). */
    public Tri certainly(String entity, String slot, Predicate pred, At at) {
        var params = new LinkedHashMap<String, Object>();
        params.put("entity", entity);
        params.put("slot", slot);
        params.putAll(pred.params());
        if (at != null) params.put("at", at.value());
        Object result = asObject(get("/certainly", params)).get("result");
        return Tri.parse((String) result);
    }

    /** FIND: entities whose region certainly lies in / possibly intersects [lo, hi]. */
    @SuppressWarnings("unchecked")
    public List<String> find(String slot, double lo, double hi, FindMode mode, At at) {
        var params = new LinkedHashMap<String, Object>();
        params.put("slot", slot);
        params.put("lo", lo);
        params.put("hi", hi);
        if (mode != null) params.put("mode", mode.wire);
        if (at != null) params.put("at", at.value());
        Object arr = get("/find", params);
        var out = new ArrayList<String>();
        for (Object e : (List<Object>) arr) out.add((String) e);
        return out;
    }

    /** JOIN: entity pairs matching a relation, each with a probability and certainty. */
    public JoinResult join(JoinPredicate predicate, JoinOptions options, At at) {
        var body = new LinkedHashMap<String, Object>();
        body.put("predicate", predicate.json());
        body.put("options", (options == null ? JoinOptions.defaults() : options).json());
        if (at != null) body.put("at", at.value());
        return JoinResult.from(asObject(post("/join", body)));
    }

    public JoinResult join(JoinPredicate predicate) {
        return join(predicate, null, null);
    }

    /** RESOLVE: plan the minimal-cost evidence to reach an entropy target. */
    public ResolvePlan resolve(ResolveRequest req) {
        return ResolvePlan.from(asObject(post("/resolve", req.json())));
    }

    // --------------------------------------------------------------- writes

    /** Append one evidence record to the log. */
    public Map<String, Object> ingest(String entity, Claim claim, String source, At at) {
        var body = new LinkedHashMap<String, Object>();
        body.put("entity", entity);
        body.put("claim", claim.json());
        body.put("source", source);
        if (at != null) body.put("at", at.value());
        return asObject(post("/ingest", body));
    }

    /** Append many records with a single group commit (one fsync). */
    public Map<String, Object> ingestBatch(List<EvidenceRecord> records) {
        var arr = new ArrayList<Object>();
        for (EvidenceRecord r : records) arr.add(r.json());
        return asObject(post("/ingest-batch", arr));
    }

    /** Register or update a source (updating re-interprets its whole history). */
    public Map<String, Object> putSource(Source source) {
        return asObject(post("/sources", source.json()));
    }

    /** GDPR erasure: physically remove all evidence from a source. */
    public Map<String, Object> forgetSource(String source) {
        return asObject(post("/forget-source", Map.of("source", source)));
    }

    /** Learn a source's decay physics from ground truth in the log. */
    public Map<String, Object> recalibrate(String source, boolean apply) {
        var body = new LinkedHashMap<String, Object>();
        body.put("source", source);
        body.put("apply", apply);
        return asObject(post("/recalibrate", body));
    }

    public Map<String, Object> registerPrior(String name, String slot, double[] weights) {
        var w = new ArrayList<Object>();
        for (double x : weights) w.add(x);
        return asObject(post("/priors/register", Map.of("name", name, "slot", slot, "weights", w)));
    }

    public Map<String, Object> usePrior(String entity, String slot, String name) {
        return asObject(post("/priors/use", Map.of("entity", entity, "slot", slot, "name", name)));
    }

    // ------------------------------------------------------------- plumbing

    private Object get(String path, Map<String, Object> params) {
        var qs = new StringBuilder();
        for (var e : params.entrySet()) {
            if (e.getValue() == null) continue;
            qs.append(qs.isEmpty() ? '?' : '&')
                .append(enc(e.getKey())).append('=').append(enc(String.valueOf(e.getValue())));
        }
        return request("GET", path + qs, null);
    }

    private Object post(String path, Object body) {
        return request("POST", path, Json.write(body));
    }

    private Object request(String method, String path, String body) {
        var builder = HttpRequest.newBuilder(URI.create(base + path)).timeout(timeout);
        if (body != null) {
            builder.header("Content-Type", "application/json")
                .method(method, HttpRequest.BodyPublishers.ofString(body, StandardCharsets.UTF_8));
        } else {
            builder.method(method, HttpRequest.BodyPublishers.noBody());
        }
        HttpResponse<String> res;
        try {
            res = http.send(builder.build(), HttpResponse.BodyHandlers.ofString(StandardCharsets.UTF_8));
        } catch (Exception e) {
            throw new NescioException(0, "request to " + base + path + " failed: " + e);
        }
        Object parsed = res.body().isEmpty() ? null : Json.read(res.body());
        if (res.statusCode() / 100 != 2) {
            String msg = parsed instanceof Map<?, ?> m && m.get("error") != null
                ? String.valueOf(m.get("error"))
                : "HTTP " + res.statusCode();
            throw new NescioException(res.statusCode(), msg);
        }
        return parsed;
    }

    private static String enc(String s) {
        return URLEncoder.encode(s, StandardCharsets.UTF_8);
    }

    @SuppressWarnings("unchecked")
    private static Map<String, Object> asObject(Object o) {
        return (Map<String, Object>) o;
    }

    // ============================================================ data types

    /** A point in time: an ISO date/datetime, or unix seconds. */
    public record At(Object value) {
        public static At date(String iso) {
            return new At(iso);
        }

        public static At unix(long seconds) {
            return new At(seconds);
        }
    }

    public enum Tri {
        TRUE, POSSIBLE, FALSE;

        static Tri parse(String s) {
            return switch (s) {
                case "true" -> TRUE;
                case "possible" -> POSSIBLE;
                default -> FALSE;
            };
        }
    }

    public enum FindMode {
        POSSIBLE("possible"), CERTAIN("certain");

        final String wire;

        FindMode(String wire) {
            this.wire = wire;
        }
    }

    /** The credible region: numeric intervals (continuous) or labels (categorical). */
    public record Region(List<double[]> intervals, List<String> values) {
        public boolean isNumeric() {
            return intervals != null;
        }

        @SuppressWarnings("unchecked")
        static Region from(Object raw) {
            var arr = (List<Object>) raw;
            if (!arr.isEmpty() && arr.get(0) instanceof List) {
                var ivs = new ArrayList<double[]>();
                for (Object o : arr) {
                    var pair = (List<Object>) o;
                    ivs.add(new double[] {num(pair.get(0)), num(pair.get(1))});
                }
                return new Region(ivs, null);
            }
            var vals = new ArrayList<String>();
            for (Object o : arr) vals.add((String) o);
            return new Region(null, vals);
        }
    }

    public record Bound(
        String entity,
        String slot,
        Region region,
        double entropyBits,
        double maxEntropyBits,
        Object mapEstimate) {

        /** 0 = knows nothing, 1 = fully collapsed. */
        public double knowledgeRatio() {
            return maxEntropyBits == 0 ? 1.0 : 1.0 - entropyBits / maxEntropyBits;
        }

        static Bound from(Map<String, Object> j) {
            return new Bound(
                (String) j.get("entity"),
                (String) j.get("slot"),
                Region.from(j.get("region")),
                num(j.get("entropy_bits")),
                num(j.get("max_entropy_bits")),
                j.get("map_estimate"));
        }
    }

    public record Source(String name, double reliability, Double halfLifeDays, boolean axiomatic) {
        public static Source decaying(String name, double reliability, double halfLifeDays) {
            return new Source(name, reliability, halfLifeDays, false);
        }

        public static Source axiom(String name) {
            return new Source(name, 1.0, null, true);
        }

        Map<String, Object> json() {
            var m = new LinkedHashMap<String, Object>();
            m.put("name", name);
            m.put("reliability", reliability);
            if (halfLifeDays != null) m.put("half_life_days", halfLifeDays);
            m.put("axiomatic", axiomatic);
            return m;
        }
    }

    /** A claim about a slot. Use the static constructors. */
    public record Claim(Map<String, Object> json) {
        public static Claim interval(String slot, double lo, double hi) {
            return new Claim(ordered("type", "interval", "slot", slot, "lo", lo, "hi", hi));
        }

        public static Claim value(String slot, String value) {
            return new Claim(ordered("type", "value", "slot", slot, "value", value));
        }

        public static Claim notValue(String slot, String value) {
            return new Claim(ordered("type", "not_value", "slot", slot, "value", value));
        }
    }

    public record EvidenceRecord(String entity, Claim claim, String source, At at) {
        Map<String, Object> json() {
            var m = new LinkedHashMap<String, Object>();
            m.put("entity", entity);
            m.put("claim", claim.json());
            m.put("source", source);
            if (at != null) m.put("at", at.value());
            return m;
        }
    }

    /** Predicate for {@link #certainly}. Use the static constructors. */
    public record Predicate(Map<String, Object> params) {
        public static Predicate gt(double value) {
            return new Predicate(ordered("op", "gt", "value", value));
        }

        public static Predicate lt(double value) {
            return new Predicate(ordered("op", "lt", "value", value));
        }

        public static Predicate between(double lo, double hi) {
            return new Predicate(ordered("op", "between", "lo", lo, "hi", hi));
        }

        public static Predicate is(String value) {
            return new Predicate(ordered("op", "is", "value", value));
        }

        public static Predicate isNot(String value) {
            return new Predicate(ordered("op", "is_not", "value", value));
        }
    }

    /** Join predicate. Use the static constructors. */
    public record JoinPredicate(Map<String, Object> json) {
        public static JoinPredicate gt(String left, String right) {
            return new JoinPredicate(ordered("op", "gt", "left", left, "right", right));
        }

        public static JoinPredicate lt(String left, String right) {
            return new JoinPredicate(ordered("op", "lt", "left", left, "right", right));
        }

        public static JoinPredicate approx(String left, String right, double tol) {
            return new JoinPredicate(ordered("op", "approx", "left", left, "right", right, "tol", tol));
        }

        public static JoinPredicate same(String left, String right) {
            return new JoinPredicate(ordered("op", "same", "left", left, "right", right));
        }
    }

    public record JoinOptions(
        String leftPrefix,
        String rightPrefix,
        Double minProbability,
        Boolean certainOnly,
        Boolean requireEvidence,
        Integer limit) {

        public static JoinOptions defaults() {
            return new JoinOptions(null, null, null, null, null, null);
        }

        public JoinOptions leftPrefix(String p) {
            return new JoinOptions(p, rightPrefix, minProbability, certainOnly, requireEvidence, limit);
        }

        public JoinOptions rightPrefix(String p) {
            return new JoinOptions(leftPrefix, p, minProbability, certainOnly, requireEvidence, limit);
        }

        public JoinOptions minProbability(double p) {
            return new JoinOptions(leftPrefix, rightPrefix, p, certainOnly, requireEvidence, limit);
        }

        public JoinOptions certainOnly(boolean c) {
            return new JoinOptions(leftPrefix, rightPrefix, minProbability, c, requireEvidence, limit);
        }

        public JoinOptions limit(int n) {
            return new JoinOptions(leftPrefix, rightPrefix, minProbability, certainOnly, requireEvidence, n);
        }

        Map<String, Object> json() {
            var m = new LinkedHashMap<String, Object>();
            if (leftPrefix != null) m.put("left_prefix", leftPrefix);
            if (rightPrefix != null) m.put("right_prefix", rightPrefix);
            if (minProbability != null) m.put("min_probability", minProbability);
            if (certainOnly != null) m.put("certain_only", certainOnly);
            if (requireEvidence != null) m.put("require_evidence", requireEvidence);
            if (limit != null) m.put("limit", limit);
            return m;
        }
    }

    public record JoinMatch(String left, String right, double probability, Tri certainty) {}

    public record JoinResult(List<JoinMatch> matches, long pairsExamined, boolean truncated) {
        @SuppressWarnings("unchecked")
        static JoinResult from(Map<String, Object> j) {
            var out = new ArrayList<JoinMatch>();
            for (Object o : (List<Object>) j.get("matches")) {
                var m = (Map<String, Object>) o;
                out.add(new JoinMatch(
                    (String) m.get("left"),
                    (String) m.get("right"),
                    num(m.get("probability")),
                    Tri.parse((String) m.get("certainty"))));
            }
            return new JoinResult(out, (long) num(j.get("pairs_examined")), (Boolean) j.get("truncated"));
        }
    }

    public record ProcurementAction(
        String name, String slot, double cost, Source source, Double answerWidth) {
        Map<String, Object> json() {
            var m = new LinkedHashMap<String, Object>();
            m.put("name", name);
            m.put("slot", slot);
            m.put("cost", cost);
            m.put("source", source.json());
            if (answerWidth != null) m.put("answer_width", answerWidth);
            return m;
        }
    }

    public record ResolveRequest(
        String entity,
        String slot,
        double targetBits,
        List<ProcurementAction> actions,
        Integer maxSteps,
        Integer mc,
        Long seed,
        At at) {

        public ResolveRequest(String entity, String slot, double targetBits, List<ProcurementAction> actions) {
            this(entity, slot, targetBits, actions, null, null, null, null);
        }

        Map<String, Object> json() {
            var m = new LinkedHashMap<String, Object>();
            m.put("entity", entity);
            m.put("slot", slot);
            m.put("target_bits", targetBits);
            var acts = new ArrayList<Object>();
            for (ProcurementAction a : actions) acts.add(a.json());
            m.put("actions", acts);
            if (maxSteps != null) m.put("max_steps", maxSteps);
            if (mc != null) m.put("mc", mc);
            if (seed != null) m.put("seed", seed);
            if (at != null) m.put("at", at.value());
            return m;
        }
    }

    public record ResolveStep(ProcurementAction action, double expectedEntropyBits, double expectedGainBits) {}

    public record ResolvePlan(
        List<ResolveStep> steps,
        double startEntropyBits,
        double plannedEntropyBits,
        Optional<Double> validatedEntropyBits,
        double totalCost) {

        @SuppressWarnings("unchecked")
        static ResolvePlan from(Map<String, Object> j) {
            var steps = new ArrayList<ResolveStep>();
            for (Object o : (List<Object>) j.get("steps")) {
                var s = (Map<String, Object>) o;
                var a = (Map<String, Object>) s.get("action");
                var src = (Map<String, Object>) a.get("source");
                var action = new ProcurementAction(
                    (String) a.get("name"),
                    (String) a.get("slot"),
                    num(a.get("cost")),
                    new Source(
                        (String) src.get("name"),
                        num(src.get("reliability")),
                        src.get("half_life_days") == null ? null : num(src.get("half_life_days")),
                        Boolean.TRUE.equals(src.get("axiomatic"))),
                    a.get("answer_width") == null ? null : num(a.get("answer_width")));
                steps.add(new ResolveStep(
                    action, num(s.get("expected_entropy_bits")), num(s.get("expected_gain_bits"))));
            }
            Object v = j.get("validated_entropy_bits");
            return new ResolvePlan(
                steps,
                num(j.get("start_entropy_bits")),
                num(j.get("planned_entropy_bits")),
                v == null ? Optional.empty() : Optional.of(num(v)),
                num(j.get("total_cost")));
        }
    }

    // -------------------------------------------------------------- helpers

    private static double num(Object o) {
        return ((Number) o).doubleValue();
    }

    private static Map<String, Object> ordered(Object... kv) {
        var m = new LinkedHashMap<String, Object>();
        for (int i = 0; i < kv.length; i += 2) m.put((String) kv[i], kv[i + 1]);
        return m;
    }
}
