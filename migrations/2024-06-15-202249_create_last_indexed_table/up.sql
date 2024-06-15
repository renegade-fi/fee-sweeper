-- Create the table that stores indexing metadata
CREATE TABLE indexing_metadata (
    latest_block INTEGER
);

-- Insert a row with the latest block number set to zero
INSERT INTO indexing_metadata (latest_block) VALUES (0);
