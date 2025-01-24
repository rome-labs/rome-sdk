-- Returns number of a latest produced Ethereum block (if any)
CREATE OR REPLACE FUNCTION latest_eth_block() RETURNS BIGINT AS $$
DECLARE
    result BIGINT;
BEGIN
    SELECT MAX(((params).number)) INTO result FROM eth_block;
    RETURN result;
END;
$$ LANGUAGE plpgsql;

------------------------------------------------------------------------------------------------------------------------

-- Returns number of produced Ethereum block (if any) by given blockhash
CREATE OR REPLACE FUNCTION get_block_number(hash_val VARCHAR(66)) RETURNS BIGINT AS $$
DECLARE
    result BIGINT;
BEGIN
    SELECT MAX(((params).number)) INTO result FROM eth_block WHERE ((params).blockhash) = hash_val;
    RETURN result;
END;
$$ LANGUAGE plpgsql;

------------------------------------------------------------------------------------------------------------------------

CREATE INDEX evm_tx_result_tx_hash ON evm_tx_result(tx_hash);

CREATE OR REPLACE FUNCTION get_transaction(tx_hash_val VARCHAR(66))
    RETURNS TABLE (
        slot_number BIGINT,
        rlp BYTEA,
        tx_result JSONB,
        receipt_params JSONB
    )
    AS $$
BEGIN
    RETURN QUERY
        SELECT
            etr.slot_number,
            et.rlp,
            etr.tx_result,
            etr.receipt_params
        FROM evm_tx et
            INNER JOIN evm_tx_result etr
                ON et.tx_hash = etr.tx_hash
        WHERE et.tx_hash = tx_hash_val;
END;
$$ LANGUAGE plpgsql;

------------------------------------------------------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION get_block_header(block_number_value BIGINT)
    RETURNS TABLE (
        slot_number BIGINT,
        slot_block_idx INTEGER,
        block_gas_used TEXT,
        gas_recipient VARCHAR(42),
        slot_timestamp BIGINT,
        blockhash VARCHAR(66),
        parent_hash VARCHAR(66),
        number BIGINT,
        block_timestamp TEXT
                  )
AS $$
BEGIN
    RETURN QUERY
        SELECT
            eb.slot_number,
            eb.slot_block_idx,
            eb.block_gas_used::TEXT,
            eb.gas_recipient,
            eb.slot_timestamp,
            (eb.params).blockhash,
            (eb.params).parent_hash,
            (eb.params).number,
            (eb.params).block_timestamp::TEXT
        FROM eth_block eb
        WHERE (eb.params).number = block_number_value;
END;
$$ LANGUAGE plpgsql;

------------------------------------------------------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION get_block_transaction_hashes(block_number_value BIGINT)
    RETURNS TABLE (
        tx_hash VARCHAR(66)
                  )
AS $$
BEGIN
    RETURN QUERY
        SELECT
            ebt.tx_hash
        FROM eth_block eb
                 INNER JOIN eth_block_txs ebt
                            ON eb.slot_number = ebt.slot_number AND eb.slot_block_idx = ebt.slot_block_idx
        WHERE (eb.params).number = block_number_value
        ORDER BY ebt.tx_idx;
END;
$$ LANGUAGE plpgsql;

------------------------------------------------------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION get_block_transactions(block_number_value BIGINT)
    RETURNS TABLE (
        tx_hash VARCHAR(66),
        rlp BYTEA,
        receipt_params JSONB
                  )
AS $$
BEGIN
    RETURN QUERY
        SELECT
            ebt.tx_hash,
            et.rlp,
            etr.receipt_params
        FROM eth_block eb
                 INNER JOIN eth_block_txs ebt
                            ON ebt.slot_number = eb.slot_number AND ebt.slot_block_idx = eb.slot_block_idx
                 INNER JOIN evm_tx_result etr
                            ON etr.slot_number = eb.slot_number AND etr.tx_hash = ebt.tx_hash
                 INNER JOIN evm_tx et
                            ON et.tx_hash = etr.tx_hash
        WHERE (eb.params).number = block_number_value
        ORDER BY ebt.tx_idx;
END;
$$ LANGUAGE plpgsql;
