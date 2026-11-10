import test from "node:test";
import assert from "node:assert/strict";
import { evaluatePurchase } from "./server.js";

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
