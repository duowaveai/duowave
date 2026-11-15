-- The router's deals and prices (src/router; docs/plans/router-and-works.md, block A, 1 and 2). A deal is one answer
-- of a model: which seat asked, which model answered and why, what it spent in tokens and in dollars, at which price.
-- Never the phrase, the answer or a file's text. `model_` in the names: deals of models, not the deals other parts of
-- the gateway may have one day. Deals older than 90 days go; their days stay in `model_deals_days` for a year. Each
-- gateway keeps its own: none of these tables travels with the settings package.
CREATE TABLE model_deals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- When the answer ended, ms since the epoch.
    at INTEGER NOT NULL,
    -- The seat: `fast`, `smart`, `backup`, `workshop`, `memory`, `app`; later `planner`, `worker`, `validator`.
    seat TEXT NOT NULL,
    -- How the call went: `brain` — the gateway called the model itself; `harness:claude-code` — a harness's run.
    via TEXT NOT NULL,
    -- Who answered: the provider's id in «Мозг» (`env` — the model of .env) and the model as it was sent.
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    -- Where it is: `home`, `mine` (one's own cloud), `cloud`.
    place TEXT NOT NULL,
    -- Why this model: `rule:command`, `rule:hard`, `rule:short`, `seat`, `backup`; later `table:<rule>`,
    -- `budget:<step>`.
    reason TEXT NOT NULL,
    -- What it belongs to: the journal's request, the work, the work's part.
    request TEXT,
    work TEXT,
    part TEXT,
    -- Tokens of all its rounds: the fresh input (neither read from the cache nor written to it), the output, read
    -- from the cache, written to it. The whole input is the three together.
    tokens_in INTEGER NOT NULL DEFAULT 0,
    tokens_out INTEGER NOT NULL DEFAULT 0,
    tokens_cache_read INTEGER NOT NULL DEFAULT 0,
    tokens_cache_write INTEGER NOT NULL DEFAULT 0,
    rounds INTEGER NOT NULL DEFAULT 0,
    -- From the first round to the end of the answer.
    ms INTEGER NOT NULL DEFAULT 0,
    -- Millionths of a dollar; empty — no price is known.
    usd_micro INTEGER,
    -- The price it was counted at, as JSON: `{input, output, cacheRead, cacheWrite, from}`, dollars per million tokens;
    -- `from` — `manual`, `provider`, `snapshot`, `local`.
    price TEXT,
    -- `key` — by a key; `plan` — by a plan's sign-in (the dollars as the key would have cost); `local` — at home.
    billing TEXT NOT NULL,
    -- `ok`, `failed`, `limit` (a plan's or a service's limit), `budget`, `cut`, `stopped`.
    outcome TEXT NOT NULL,
    -- A part's check: `passed`, `fixed`, `failed`. The plan's `check` — a word SQL keeps for itself.
    checked TEXT
);

CREATE INDEX model_deals_at ON model_deals (at);
CREATE INDEX model_deals_work ON model_deals (work, at);

-- The days of the deals: one row per day (by UTC), seat, provider, model, way of the call and billing, summed as each
-- deal is written. Kept a year: the deals themselves go after 90 days.
CREATE TABLE model_deals_days (
    day TEXT NOT NULL,
    seat TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    via TEXT NOT NULL,
    billing TEXT NOT NULL,
    deals INTEGER NOT NULL DEFAULT 0,
    tokens_in INTEGER NOT NULL DEFAULT 0,
    tokens_out INTEGER NOT NULL DEFAULT 0,
    tokens_cache_read INTEGER NOT NULL DEFAULT 0,
    tokens_cache_write INTEGER NOT NULL DEFAULT 0,
    rounds INTEGER NOT NULL DEFAULT 0,
    ms INTEGER NOT NULL DEFAULT 0,
    -- The dollars of the deals with a price, and how many had none.
    usd_micro INTEGER NOT NULL DEFAULT 0,
    unpriced INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day, seat, provider, model, via, billing)
) WITHOUT ROWID;

-- The models' prices beside the snapshot built into the program (src/router/prices.json): written by hand in the
-- panel (`manual`) or said by the provider's own list of models (`provider`). Taken in that order, then the snapshot.
CREATE TABLE model_prices (
    -- The provider's id in «Мозг» and the model as it is sent.
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    source TEXT NOT NULL,
    -- Dollars per million tokens: the fresh input, the output, read from the cache, written to it; empty — unknown.
    input REAL,
    output REAL,
    cache_read REAL,
    cache_write REAL,
    -- The context's limit in tokens.
    context INTEGER,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (provider, model, source)
) WITHOUT ROWID;
