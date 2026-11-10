import http from "node:http";

const PORT = Number(process.env.PORT || 8787);
const PRODUCTS = [
  {
    plan: "monthly",
    productId: process.env.GUMROAD_MONTHLY_PRODUCT_ID || "Niyme89XCcFC44ShhEPPrQ=="
  },
  {
    plan: "permanent",
    productId: process.env.GUMROAD_PERMANENT_PRODUCT_ID || "ksPYCeAJW8TtmOjsBRQ0OA=="
  }
];

function json(res, status, body) {
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store"
  });
  res.end(JSON.stringify(body));
}

function denied(status) {
  return { licensed: false, plan: null, status, valid_until: null };
}

export function evaluatePurchase(purchase, product, now = Date.now()) {
  if (!purchase || purchase.product_id !== product.productId) return denied("invalid");

  if (purchase.refunded === true || purchase.chargebacked === true ||
      (purchase.disputed === true && purchase.dispute_won !== true)) {
    return denied("revoked");
  }

  if (product.plan === "permanent") {
    return {
      licensed: true,
      plan: "permanent",
      status: "permanent",
      valid_until: null
    };
  }

  if (product.plan !== "monthly" || purchase.recurrence !== "monthly") {
    return denied("invalid");
  }

  // Gumroad timestamps are the end of membership access, not when cancellation
  // was requested. A future timestamp means the paid period is still active.
  const endFields = [
    ["subscription_cancelled_at", "cancelled_active"],
    ["subscription_ended_at", "ending_active"],
    ["subscription_failed_at", "payment_failed_active"]
  ];

  let validUntil = null;
  let activeStatus = "active";

  for (const [field, status] of endFields) {
    const value = purchase[field];
    if (value == null || value === "") continue;

    const timestamp = Date.parse(value);
    if (!Number.isFinite(timestamp) || timestamp <= now) return denied("expired");

    validUntil = value;
    activeStatus = status;
    break;
  }

  return {
    licensed: true,
    plan: "monthly",
    status: activeStatus,
    valid_until: validUntil
  };
}

async function verifyAgainstProduct(licenseKey, product) {
  const form = new URLSearchParams({
    product_id: product.productId,
    license_key: licenseKey,
    increment_uses_count: "false"
  });

  const response = await fetch("https://api.gumroad.com/v2/licenses/verify", {
    method: "POST",
    body: form,
    signal: AbortSignal.timeout(15000)
  });

  // Gumroad returns 404 for a key that does not belong to the supplied product.
  if (response.status === 404) return null;
  if (!response.ok) throw new Error("Gumroad verification unavailable");

  const payload = await response.json();
  if (payload.success !== true) return null;

  return payload.purchase || null;
}

export async function verifyLicense(licenseKey) {
  for (const product of PRODUCTS) {
    const purchase = await verifyAgainstProduct(licenseKey, product);
    if (!purchase) continue;

    // A key matched a product. Do not fall through to a different product if
    // its entitlement is revoked or expired.
    return evaluatePurchase(purchase, product);
  }

  return denied("invalid");
}

const server = http.createServer(async (req, res) => {
  if (req.method === "GET" && req.url === "/health") {
    return json(res, 200, { ok: true });
  }
  if (req.method !== "POST" || req.url !== "/v1/verify") {
    return json(res, 404, { error: "not_found" });
  }

  let raw = "";
  for await (const chunk of req) {
    raw += chunk;
    if (raw.length > 4096) return json(res, 413, { error: "request_too_large" });
  }

  let input;
  try {
    input = JSON.parse(raw);
  } catch {
    return json(res, 400, { error: "invalid_json" });
  }

  const key = typeof input.license_key === "string" ? input.license_key.trim() : "";
  if (!key || key.length > 256) {
    return json(res, 400, { error: "invalid_license_key" });
  }

  try {
    return json(res, 200, await verifyLicense(key));
  } catch {
    // Deliberately do not log the submitted key or Gumroad's purchase payload.
    return json(res, 503, { error: "verification_unavailable" });
  }
});

if (process.env.NODE_ENV !== "test") {
  server.listen(PORT, "0.0.0.0", () => {
    console.log(`Blaze licensing service listening on ${PORT}`);
  });
}
