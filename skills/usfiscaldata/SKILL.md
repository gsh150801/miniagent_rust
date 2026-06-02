---
name: usfiscaldata
description: Query the U.S. Treasury Fiscal Data API for federal financial data including national debt, government spending, revenue, interest rates, exchange rates, and savings bonds. Access 54 datasets and 182 
triggers:
  - statistical test
  - usfiscaldata
  - analyze data
  - data analysis
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# U.S. Treasury Fiscal Data API

Free, open REST API from the U.S. Department of the Treasury for federal financial data. No API key or registration required.

**Base URL:** `https://api.fiscaldata.treasury.gov/services/api/fiscal_service`

## Quick Start

```python
import requests
import pandas as pd

BASE_URL = "https://api.fiscaldata.treasury.gov/services/api/fiscal_service"

# Get the current national debt (Debt to the Penny)
resp = requests.get(f"{BASE_URL}/v2/accounting/od/debt_to_penny", params={
    "sort": "-record_date",
    "page[size]": 1
})
data = resp.json()["data"][0]
print(f"Total public debt as of {data['record_date']}: ${float(data['tot_pub_debt_out_amt']):,.0f}")
```

```python
# Get Treasury exchange rates for recent quarters
resp = requests.get(f"{BASE_URL}/v1/accounting/od/rates_of_exchange", params={
    "fields": "country_currency_desc,exchange_rate,record_date",
    "filter": "record_date:gte:2024-01-01",
    "sort": "-record_date",
    "page[size]": 100
})
df = pd.DataFrame(resp.json()["data"])
```

## Authentication

None required. The API is fully open and free.

## Core Parameters

| Parameter | Example | Description |
|-----------|---------|-------------|
| `fields=` | `fields=record_date,tot_pub_debt_out_amt` | Select specific columns |
| `filter=` | `filter=record_date:gte:2024-01-01` | Filter records |
| `sort=` | `sort=-record_date` | Sort (prefix `-` for descending) |
| `format=` | `format=json` | Output format: `json`, `csv`, `xml` |
| `page[size]=` | `page[size]=100` | Records per page (default 100) |
| `page[number]=` | `page[number]=2` | Page index (starts at 1) |

**Filter operators:** `lt`, `lte`, `gt`, `gte`, `eq`, `in`

```python
# Multiple filters separated by comma
"filter=country_currency_desc:in:(Canada-Dollar,Mexico-Peso),record_date:gte:2024-01-01"
```

## Key Datasets & Endpoints

### Debt

| Dataset | Endpoint | Frequency |
|---------|----------|-----------|
| Debt to the Penny | `/v2/accounting/od/debt_to_penny` | Daily |
| Historical Debt Outstanding | `/v2/accounting/od/historical_debt_outstanding` | Annual |
| Schedules of Federal Debt | `/v1/accounting/od/schedules_fed_debt` | Monthly |

### Daily & Monthly Statements

| Dataset | Endpoint | Frequency |
|---------|----------|-----------|
| DTS Operating Cash Balance | `/v1/accounting/dts/operating_cash_balance` | Daily |
| DTS Deposits & Withdrawals | `/v1/accounting/dts/deposits_withdrawals_operating_cash` | Daily |
| Monthly Treasury Statement (MTS) | `/v1/accounting/mts/mts_table_1` (16 tables) | Monthly |

### Interest Rates & Exchange

| Dataset | Endpoint | Frequency |
|---------|----------|-----------|
| Average Interest Rates on Treasury Securities | `/v2/accounting/od/avg_interest_rates` | Monthly |
| Treasury Reporting Rates of Exchange | `/v1/accounting/od/rates_of_exchange` | Quarterly |
| Interest Expense on Public Debt | `/v2/accounting/od/interest_expense` | Monthly |

### Securities & Auctions

| Dataset | Endpoint | Frequenc

... (truncated from original)