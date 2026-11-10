import test from "node:test";
import assert from "node:assert/strict";
import { evaluatePurchase, verifyLicense } from "./server.js";

const MONTHLY = { plan: "monthly", productId: "Niyme89XCcFC44ShhEPPrQ==" };
const PERMANENT = { plan: "permanent", productId: "ksPYCeAJW8TtmOjsBRQ0OA==" };
const base = {
  product_id: MONTHLY.productId,
  recurrence: "monthly",
  refunded: false,
  chargebacked: false,
  disputed: false
};

test("active monthly subscription is licensed", () => {
  assert.deepEqual(evaluatePurchase(base, MONTHLY, Date.parse("2026-11-10T00:00:00Z")), {
    licensed: true, plan: "monthly", status: "active", valid_until: null
  });
});

test("cancelled monthly subscription stays licensed until its paid-through date", () => {
  const purchase = { ...base, subscription_cancelled_at: "2026-12-10T16:08:18Z" };
  assert.deepEqual(evaluatePurchase(purchase, MONTHLY, Date.parse("2026-11-10T00:00:00Z")), {
    licensed: true, plan: "monthly", status: "cancelled_active",
    valid_until: "2026-12-10T16:08:18Z"
  });
});

test("monthly subscription expires at the exact membership end timestamp", () => {
  const purchase = { ...base, subscription_cancelled_at: "2026-12-10T16:08:18Z" };
  assert.equal(evaluatePurchase(purchase, MONTHLY, Date.parse("2026-12-10T16:08:18Z")).licensed, false);
});

test("ended and failed monthly memberships are denied after their end date", () => {
  assert.equal(evaluatePurchase({
    ...base, subscription_ended_at: "2026-11-09T00:00:00Z"
  }, MONTHLY, Date.parse("2026-11-10T00:00:00Z")).licensed, false);

  assert.equal(evaluatePurchase({
    ...base, subscription_failed_at: "2026-11-09T00:00:00Z"
  }, MONTHLY, Date.parse("2026-11-10T00:00:00Z")).licensed, false);
});

test("malformed subscription end dates fail closed", () => {
  assert.equal(evaluatePurchase({
    ...base, subscription_cancelled_at: "not-a-date"
  }, MONTHLY).licensed, false);
});

test("monthly product must actually be a monthly recurrence", () => {
  assert.equal(evaluatePurchase({ ...base, recurrence: null }, MONTHLY).licensed, false);
});

test("permanent product remains licensed without subscription timestamps", () => {
  assert.deepEqual(evaluatePurchase({
    product_id: PERMANENT.productId,
    recurrence: null,
    refunded: false,
    chargebacked: false,
    disputed: false
  }, PERMANENT), {
    licensed: true, plan: "permanent", status: "permanent", valid_until: null
  });
});

test("permanent product does not expire based on subscription fields", () => {
  assert.equal(evaluatePurchase({
    product_id: PERMANENT.productId,
    refunded: false,
    chargebacked: false,
    disputed: false,
    subscription_ended_at: "2020-01-01T00:00:00Z"
  }, PERMANENT).licensed, true);
});

test("refunds, chargebacks and unresolved disputes revoke both plans", () => {
  for (const product of [MONTHLY, PERMANENT]) {
    const recurrence = product.plan === "monthly" ? "monthly" : null;
    for (const field of ["refunded", "chargebacked"]) {
      assert.equal(evaluatePurchase({
        product_id: product.productId, recurrence, [field]: true
      }, product).licensed, false);
    }
    assert.equal(evaluatePurchase({
      product_id: product.productId, recurrence, disputed: true, dispute_won: false
    }, product).licensed, false);
  }
});

test("a product ID mismatch is denied", () => {
  assert.equal(evaluatePurchase(base, PERMANENT).licensed, false);
});

test("a permanent key is checked against both products and resolves as permanent", async () => {
  const productChecks = [];
  const fetchImpl = async (_url, options) => {
    const form = options.body;
    const productId = form.get("product_id");
    productChecks.push(productId);
    if (productId === MONTHLY.productId) {
      return { status: 404, ok: false };
    }
    return {
      status: 200,
      ok: true,
      json: async () => ({
        success: true,
        purchase: {
          product_id: PERMANENT.productId,
          refunded: false,
          chargebacked: false,
          disputed: false
        }
      })
    };
  };

  assert.deepEqual(await verifyLicense("permanent-key", false, fetchImpl), {
    licensed: true, plan: "permanent", status: "permanent", valid_until: null
  });
  assert.deepEqual(productChecks, [MONTHLY.productId, PERMANENT.productId]);
});

test("a monthly key resolves on the first product check without trying permanent", async () => {
  const productChecks = [];
  const fetchImpl = async (_url, options) => {
    const form = options.body;
    productChecks.push(form.get("product_id"));
    return {
      status: 200,
      ok: true,
      json: async () => ({
        success: true,
        purchase: { ...base, subscription_cancelled_at: "2026-12-10T16:08:18Z" }
      })
    };
  };

  assert.equal((await verifyLicense("monthly-key", false, fetchImpl)).status, "cancelled_active");
  assert.deepEqual(productChecks, [MONTHLY.productId]);
});

test("activation increments Gumroad uses once, after a valid entitlement is established", async () => {
  const usesFlags = [];
  const fetchImpl = async (_url, options) => {
    const form = options.body;
    usesFlags.push(form.get("increment_uses_count"));
    return {
      status: 200,
      ok: true,
      json: async () => ({ success: true, purchase: base })
    };
  };

  assert.equal((await verifyLicense("monthly-key", true, fetchImpl)).licensed, true);
  assert.deepEqual(usesFlags, ["false", "true"]);
});

test("routine checks never increment Gumroad uses", async () => {
  const usesFlags = [];
  const fetchImpl = async (_url, options) => {
    usesFlags.push(options.body.get("increment_uses_count"));
    return { status: 200, ok: true, json: async () => ({ success: true, purchase: base }) };
  };

  await verifyLicense("monthly-key", false, fetchImpl);
  assert.deepEqual(usesFlags, ["false"]);
});

test("Gumroad service errors fail as errors, not as invalid purchases", async () => {
  const fetchImpl = async () => ({ status: 503, ok: false });
  await assert.rejects(() => verifyLicense("monthly-key", false, fetchImpl));
});
