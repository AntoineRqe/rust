# How to use the web server

## Start the server

```bash
cd tools
python fix_web_terminal.py --https --password <yourSecretPassword>
```

All options for the `fix_web_terminal.py` script can be found by running:

```bash
usage: fix_web_terminal.py [-h] [--http-port HTTP_PORT] [--fix-host FIX_HOST] [--fix-port FIX_PORT] [--password PASSWORD] [--cert CERT] [--key KEY] [--https]

FIX 4.2 Web Terminal

options:
  -h, --help            show this help message and exit
  --http-port HTTP_PORT
                        HTTP/HTTPS port (default: 7654)
  --fix-host FIX_HOST   FIX server host
  --fix-port FIX_PORT   FIX server port
  --password PASSWORD   Password for browser login (recommended when exposed to internet)
  --cert CERT           TLS cert file (.pem). If omitted with --https, auto-generates self-signed.
  --key KEY             TLS key file (.pem)
  --https               Enable HTTPS (TLS)
```

## Access the web terminal

Open your web browser and navigate to `https://localhost:<port>`. You will be prompted to enter the password you set when starting the server. After entering the correct password, you will have access to the web terminal.

## Accessing from internet

If you want to access the web terminal from the internet, you need to ensure that your server is accessible from outside your local network. This typically involves:

1. **Port Forwarding**: Configure your router to forward the port you specified (default is 7654) to the internal IP address of the machine running the web server.
2. **Firewall Rules**: Ensure that your server's firewall allows incoming connections on the specified port.
3. **Use a Public IP or Domain**: Instead of `localhost`, you will need to use your server's public IP address or a domain name that points to it.

**Important Security Note**: Exposing the web terminal to the internet can pose security risks. Always use a strong password, and consider additional security measures such as IP whitelisting or VPN access to restrict who can connect to the server.

### Get Public IP Address

```bash
curl ifconfig.me
curl -4 ifconfig.me
```

### Port Forwarding Example

Connect to your router's admin interface and look for the port forwarding section. Create a new port forwarding rule that forwards the external port (e.g., 7654) to the internal IP address of your server on the same port.

![./images/port_forwarding.png](./images/port_forwarding.png)

### Firewall Rule Example

```bash
# Allow port 7654 through the firewall
sudo ufw allow 7654/tcp
```

### Connecting to the Web Terminal

Once you have set up port forwarding and firewall rules, you can access the web terminal from any device with internet access by navigating to `https://<your-public-ip>:<port>`. Remember to replace `<your-public-ip>` with your actual public IP address and `<port>` with the port number you configured.


## Daily AAPL SELL automation

The `daily_aapl_sell.py` script can place a daily SELL order on simulator markets (NASDAQ, NYSE, and optionally IEX Cloud) at a specific Paris time.

Default behavior:
- Run every day at `09:00` in `Europe/Paris`
- Fetch AAPL prices from NASDAQ + NYSE public quote endpoints
- Optionally, fetch AAPL price from IEX Cloud (see below)
- Send one SELL limit order to each configured market TCP endpoint
- Quantity: `100`

### Quick start

Dry-run once:

```bash
cd tools
python daily_aapl_sell.py --once --dry-run
```

Run immediately once (sends orders):

```bash
cd tools
python daily_aapl_sell.py --once
```

Run as a daily scheduler:

```bash
cd tools
python daily_aapl_sell.py
```



### Using IEX Cloud or TradeWatch as a price source

**IEX Cloud:**
Register for a free API key at https://iexcloud.io and export your publishable token:

```bash
export IEX_CLOUD_API_TOKEN=sk_your_token_here
```

Then run:

```bash
python daily_aapl_sell.py --iex-market-name IEX
```

This will fetch the AAPL price from IEX Cloud and send a SELL order to the market endpoint named `IEX` in your config.

**TradeWatch:**
Register for an API key at https://dash.tradewatch.io/register and export your key:

```bash
export TRADEWATCH_API_KEY=tw_live_your_token_here
```

Then run:

```bash
python daily_aapl_sell.py --tradewatch-market-name TRADEWATCH
```

This will fetch the AAPL price from TradeWatch and send a SELL order to the market endpoint named `TRADEWATCH` in your config.

### Useful options

```bash
python daily_aapl_sell.py --help
```



Important options:
- `--config`: path to market config JSON (default: `crates/config/default.json`)
- `--symbol`: ticker (default: `AAPL`)
- `--qty`: quantity (default: `100`)
- `--hour` / `--minute`: schedule time (default: `09:00`)
- `--timezone`: scheduler timezone (default: `Europe/Paris`)
- `--nasdaq-market-name` / `--nyse-market-name`: market names as defined in config (defaults: `NASDAQ`, `NYSE`)
- `--iex-market-name`: market name for IEX Cloud in config (optional, enables IEX price fetch if set)
- `--tradewatch-market-name`: market name for TradeWatch in config (optional, enables TradeWatch price fetch if set)
- `--dry-run`: prints actions without sending FIX orders

### Notes

- The script uses direct FIX TCP order submission to each market endpoint from config.
- If exchange endpoints are temporarily unavailable, that run will fail and retry on the next scheduled day.
- NYSE `quotes/filter` sometimes returns metadata only (no live price) for some symbols. When NYSE indicates an `XNGS:*` instrument (e.g. `AAPL`), the script automatically falls back to NASDAQ quote data for the price value.
- IEX Cloud requires a free API key (see tools/iex_api.py). If `--iex-market-name` is set, the script will fetch the price from IEX Cloud and send an order to the corresponding market endpoint.
- TradeWatch requires an API key (see tools/tradewatch_api.py). If `--tradewatch-market-name` is set, the script will fetch the price from TradeWatch and send an order to the corresponding market endpoint.