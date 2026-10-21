# Shared server API

All `/api/` routes require `Authorization: Bearer CLIENT_KEY`. Windows and Kindle keys are independent. The admin key is only for starting server OAuth and cannot control the bulb through the client routes. There is no unauthenticated control endpoint or browser CORS allowance.

Use HTTPS through a reverse proxy on Oracle. Keep the raw server port private. No local dashboard website is included; Windows and KOReader are the interfaces.

| Method | Endpoint | JSON body |
| --- | --- | --- |
| GET | `/health` | None; process liveness only, no authentication or bulb reachability check |
| GET | `/api/bulb` | None; cloud state may be cached for up to 3 seconds |
| GET | `/api/bulb?fresh=true` | None; bypass server cache |
| POST | `/api/bulb/power` | `{"on":true}` or `{"on":false}` |
| POST | `/api/bulb/brightness` | `{"percent":50}`; integer 0–100 |
| POST | `/api/bulb/colour` | `{"r":255,"g":0,"b":0}`; integers 0–255 |
| POST | `/api/bulb/temperature` | `{"kelvin":5000}`; only 2700 and 5000 presets exposed |
| POST | `/api/bulb/preset` | `{"name":"red"}`; red, green, blue, purple, warm, white |

Colour requests convert RGB to hue/saturation and preserve the current brightness. RGB black requests off. White presets select temperature and preserve brightness. Colour and white requests read power first; if it is off, they send a separate on command after the colour command is accepted, with a 500 ms gap. They do not send on when power is already on or unknown. A failed read prevents the write; a failed wake can leave the colour changed while power remains off. Requests are never automatically retried. Use the brightness endpoint to change the level explicitly; it does not explicitly send on. Setting brightness to zero is not a substitute for the off command.

An accepted command returns HTTP 200:

```json
{"status":"accepted","confirmed":false,"demo":false}
```

`confirmed` is deliberately false: SmartThings acceptance does not establish physical completion. Multi-command operations are batched upstream and are not atomic or guaranteed to execute in a particular order. After a brief delay, read fresh status. Do not automatically replay a whole operation after a timeout or partial failure.

Example status (nullable values indicate unknown attributes):

```json
{
  "power": true,
  "brightness": 50,
  "hue": 0.0,
  "saturation": 100.0,
  "temperature": 5000,
  "online": true,
  "timestamp": "2026-10-20T23:00:00Z",
  "demo": false
}
```

`timestamp` is the upstream **power attribute** timestamp, not the timestamp of every field or a guarantee that data is fresh. `temperature` can retain its previous white setting while colour mode is active. The additive `colour_mode` field is `white`, `colour`, or null: it infers the active mode from the newest colour versus temperature event timestamp when both are available. Hue and saturation can also retain old values in white mode. `online` can be unknown and depends on the integration's health reporting.

## Example

```sh
read -r -s -p 'Client key: ' BULB_KEY; echo
curl --fail-with-body -H "Authorization: Bearer $BULB_KEY" \
  https://bulb.example.com/api/bulb
curl --fail-with-body -H "Authorization: Bearer $BULB_KEY" \
  -H 'Content-Type: application/json' -d '{"percent":50}' \
  https://bulb.example.com/api/bulb/brightness
unset BULB_KEY
```

## Errors

- 401: missing/wrong client key, or wrong admin key for the admin endpoint.
- 413: request exceeds 4096 bytes.
- 422: malformed or unsupported input.
- 502: upstream request failed or SmartThings did not return acceptance for every command. The bulb's outcome can be unknown; read status.
- 503: the server lacks valid SmartThings authorization or token refresh failed.

The server serializes control requests and coalesces short-lived cached status reads. It does not poll continuously, retry commands automatically, or expose upstream secrets in error responses.

## Administrative OAuth endpoints

`POST /admin/oauth/start` with the admin Bearer key returns an `authorize_url`. Open it in your own browser and authorize your own registered SmartThings app. The URL has a one-use random state, valid for ten minutes. The unauthenticated callback at `/oauth/callback` accepts only a matching pending state and exchanges the code server-side. Tokens are saved under `data/tokens.json`; refresh is performed when needed, not by a polling loop. Client keys cannot initiate OAuth.

Detailed setup is in [deploy/ORACLE.md](deploy/ORACLE.md).
