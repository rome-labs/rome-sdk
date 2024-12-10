use diesel_derive_enum::DbEnum;

#[derive(DbEnum, Debug, PartialEq)]
pub enum SlotStatus {
    Processed,
    Confirmed,
    Finalized,
}
