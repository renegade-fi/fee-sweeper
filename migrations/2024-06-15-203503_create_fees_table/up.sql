-- Stores fees and index by mint, amount
CREATE TABLE fees(
    id SERIAL PRIMARY KEY,
    tx_hash TEXT NOT NULL,
    mint TEXT NOT NULL,
    amount NUMERIC NOT NULL,
    blinder NUMERIC NOT NULL,
    receiver TEXT NOT NULL
);

CREATE INDEX idx_fees_mint ON fees(mint);
CREATE INDEX idx_fees_amount ON fees(amount);
