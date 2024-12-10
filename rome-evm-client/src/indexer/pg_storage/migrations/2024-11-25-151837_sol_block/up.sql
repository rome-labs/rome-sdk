CREATE TYPE SlotStatus AS ENUM ('Processed', 'Confirmed', 'Finalized');

CREATE TABLE sol_slot (
    slot_number BIGINT PRIMARY KEY,
    parent_slot BIGINT NOT NULL,
    status SlotStatus
);

CREATE TABLE sol_block (
    slot_number BIGINT PRIMARY KEY,

    CONSTRAINT fk_slot_number
        FOREIGN KEY (slot_number)
            REFERENCES sol_slot(slot_number) ON DELETE CASCADE,

    block_data BYTEA DEFAULT NULL
);

CREATE PROCEDURE set_block(
    slot_number_val BIGINT,
    parent_slot_val BIGINT,
    block BYTEA
)  LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO sol_slot (slot_number, parent_slot, status)
    VALUES (slot_number_val, parent_slot_val, 'Confirmed'::SlotStatus)
    ON CONFLICT DO NOTHING;

    INSERT INTO sol_block (slot_number, block_data)
    VALUES (slot_number_val, block)
    ON CONFLICT DO NOTHING;
END;
$$;

CREATE FUNCTION get_block(
    slot_number_val BIGINT
) RETURNS BYTEA AS $$
DECLARE
    result BYTEA;
BEGIN
    SELECT block_data INTO result FROM sol_block WHERE slot_number = slot_number_val;
    RETURN result;
END;
$$ LANGUAGE plpgsql;

CREATE PROCEDURE retain_from_slot(
    from_slot BIGINT
)  LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM sol_slot WHERE slot_number < from_slot;
END;
$$;

CREATE FUNCTION get_last_slot() RETURNS BIGINT AS $$
DECLARE
    result BIGINT;
BEGIN
    SELECT MAX(slot_number) INTO result FROM sol_block;
    RETURN result;
END;
$$ LANGUAGE plpgsql;