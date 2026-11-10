# Blaze licensing service

Server-side verification for the two Gumroad products used by Blaze:

| Product | Gumroad product ID | Entitlement |
|---|---|---|
| Blaze Monthly | `Niyme89XCcFC44ShhEPPrQ==` | Monthly subscription |
| Blaze Permanent | `ksPYCeAJW8TtmOjsBRQ0OA==` | Permanent licence |

The service tries the submitted key against the Monthly product first, then the Permanent product. Gumroad requests always use `increment_uses_count=false`. No Gumroad secret is required or sent: the license verification endpoint uses the product ID and license key.

## Run locally

Requires Node.js 20+.

```sh
cd licensing-service
npm test
GUMROAD_MONTHLY_PRODUCT_ID='Niyme89XCcFC44ShhEPPrQ==' \
GUMROAD_PERMANENT_PRODUCT_ID='ksPYCeAJW8TtmOjsBRQ0OA==' \
npm start
```

`GET /health` returns a health check. `POST /v1/verify` accepts JSON `{"license_key":"..."}` and returns only the entitlement:

```json
{"licensed":true,"plan":"monthly","status":"cancelled_active","valid_until":"2026-12-10T16:08:18Z"}
```

For a permanent key, the response is `{"licensed":true,"plan":"permanent","status":"permanent","valid_until":null}`.

## Entitlement rules

- **Monthly:** must match the Monthly product and report `recurrence: "monthly"`. Active memberships are allowed. Cancellation, fixed-length end, or failed-renewal timestamps are treated as the scheduled end of access: a future date remains active, and access is denied at or after that date.
- **Permanent:** must match the Permanent product. It has no subscription expiry check.
- **Both:** refunded purchases, chargebacks, and unresolved disputes are denied.
- Invalid keys return an unlicensed result. Gumroad/network failures return HTTP 503; the desktop app fails closed.

## Deploy

Deploy this directory as a Node 20+ HTTPS web service. Set the two product ID environment variables above. Build the native app with `BLAZE_LICENSE_API_URL` set to the service's absolute HTTPS `/v1/verify` endpoint. Do not put provider secrets or license keys in source control.
