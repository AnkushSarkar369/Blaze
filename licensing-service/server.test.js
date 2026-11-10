import test from "node:test";
import assert from "node:assert/strict";
import { evaluatePurchase } from "./server.js";

const PRODUCT = "Niyme89XCcFC44ShhEPPrQ==";
const base = { product_id: PRODUCT, recurrence: "monthly", refunded: false, chargebacked: false, disputed: false };

test("active subscription is licensed", () => {
  assert.equal(evaluatePurchase(base, Date.parse("2026-11-10T00:00:00Z")).licensed, true);
});
test("cancelled subscription remains licensed through paid-through date", () => {
  const p = { ...base, subscription_cancelled_at: "2026-12-10T16:08:18Z" };
  assert.deepEqual(evaluatePurchase(p, Date.parse("2026-11-10T00:00:00Z")), {
    licensed: true, plan: "monthly", status: "cancelled_active", valid_until: "2026-12-10T16:08:18Z"
  });
});
test("cancelled subscription expires at the scheduled end timestamp", () => {
  const p = { ...base, subscription_cancelled_at: "2026-12-10T16:08:18Z" };
  assert.equal(evaluatePurchase(p, Date.parse("2026-12-10T16:08:18Z")).licensed, false);
});
test("ended, failed, refunded, disputed and wrong-product purchases are denied", () => {
  assert.equal(evaluatePurchase({ ...base, subscription_ended_at: "2026-11-09T00:00:00Z" }).licensed, false);
  assert.equal(evaluatePurchase({ ...base, subscription_failed_at: "2026-11-09T00:00:00Z" }).licensed, false);
  assert.equal(evaluatePurchase({ ...base, refunded: true }).licensed, false);
  assert.equal(evaluatePurchase({ ...base, disputed: true }).licensed, false);
  assert.equal(evaluatePurchase({ ...base, product_id: "wrong" }).licensed, false);
});
