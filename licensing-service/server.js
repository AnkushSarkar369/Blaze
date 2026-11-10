import http from "node:http";

const PORT = Number(process.env.PORT || 8787);
const PRODUCT_ID = process.env.GUMROAD_PRODUCT_ID;
const VERIFY_SECRET = process.env.GUMROAD_VERIFY_SECRET || "";

function json(res, status, body) {
  res.writeHead(status, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" });
  res.end(JSON.stringify(body));
}

export function evaluatePurchase(purchase, now = Date.now()) {
  const denied = (status) => ({ licensed: false, plan: "monthly", status, valid_until: null });
  if (!purchase || purchase.product_id !== PRODUCT_ID || purchase.recurrence !== "monthly") return denied("invalid");
  if (purchase.refunded === true || purchase.chargebacked === true ||
      (purchase.disputed === true && purchase.dispute_won !== true)) return denied("revoked");

  const expired = (value) => {
    if (value == null || value === "") return false;
    const time = Date.parse(value);
    return !Number.isFinite(time) || time <= now;
  };
  if (expired(purchase.subscription_ended_at) || expired(purchase.subscription_failed_at) ||
      expired(purchase.subscription_cancelled_at)) return denied("expired");

  return {
    licensed: true,
    plan: "monthly",
    status: purchase.subscription_cancelled_at ? "cancelled_active" : "active",
    valid_until: purchase.subscription_cancelled_at || null
  };
}

async function verifyLicense(licenseKey) {
  const form = new URLSearchParams({
    product_id: PRODUCT_ID,
    license_key: licenseKey,
    increment_uses_count: "false"
  });
  if (VERIFY_SECRET) form.set("verify_secret", VERIFY_SECRET);

  const response = await fetch("https://api.gumroad.com/v2/licenses/verify", {
    method: "POST", body: form, signal: AbortSignal.timeout(15000)
  });
  if (!response.ok) throw new Error("Gumroad verification unavailable");
  const payload = await response.json();
  if (payload.success !== true) return { licensed: false, plan: "monthly", status: "invalid", valid_until: null };
  return evaluatePurchase(payload.purchase);
}

const server = http.createServer(async (req, res) => {
  if (req.method === "GET" && req.url === "/health") return json(res, 200, { ok: true });
  if (req.method !== "POST" || req.url !== "/v1/verify") return json(res, 404, { error: "not_found" });
  if (!PRODUCT_ID) return json(res, 503, { error: "licensing_not_configured" });

  let raw = "";
  for await (const chunk of req) {
    raw += chunk;
    if (raw.length > 4096) return json(res, 413, { error: "request_too_large" });
  }
  let input;
  try { input = JSON.parse(raw); } catch { return json(res, 400, { error: "invalid_json" }); }
  const key = typeof input.license_key === "string" ? input.license_key.trim() : "";
  if (!key || key.length > 256) return json(res, 400, { error: "invalid_license_key" });

  try {
    return json(res, 200, await verifyLicense(key));
  } catch {
    return json(res, 503, { error: "verification_unavailable" });
  }
});

if (process.env.NODE_ENV !== "test") {
  server.listen(PORT, "0.0.0.0", () => console.log(`Blaze licensing service listening on ${PORT}`));
}
