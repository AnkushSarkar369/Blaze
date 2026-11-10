# Blaze licensing service

Server-side Gumroad verification for Blaze Monthly. Gumroad product configuration and API communication remain on the server; the desktop client receives only a minimal entitlement result.

## Local run

Requires Node.js 20+.

```sh
cd licensing-service
cp .env.example .env
# Export the variables from .env in your shell.
export GUMROAD_PRODUCT_ID='Niyme89XCcFC44ShhEPPrQ=='
npm test
npm start
```

`GET /health` returns a health check. `POST /v1/verify` accepts JSON `{"license_key":"..."}` and returns `licensed`, `plan`, `status`, and `valid_until`. License keys and Gumroad purchase details are never written to logs or returned to the client.

## Deploy

Deploy this directory as a Node 20+ web service. Configure `GUMROAD_PRODUCT_ID` and optionally `GUMROAD_VERIFY_SECRET` as host environment variables. Serve behind HTTPS. Set `BLAZE_LICENSE_API_URL` at build time for the desktop client to the service's absolute `/v1/verify` URL. Do not put the Gumroad verification secret in the desktop application.

Subscription rules: active subscriptions are allowed; cancelled subscriptions remain allowed until Gumroad's `subscription_cancelled_at` paid-through timestamp; ended/failed subscriptions, refunds, chargebacks, unresolved disputes, wrong product IDs and non-monthly purchases are denied. API/network errors fail closed in the app.
