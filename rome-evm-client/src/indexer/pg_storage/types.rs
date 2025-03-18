use diesel_derive_enum::DbEnum;

#[derive(DbEnum, Debug, PartialEq)]
pub enum SlotStatus {
    #[db_rename = "Processed"]
    Processed,
    #[db_rename = "Confirmed"]
    Confirmed,
    #[db_rename = "Finalized"]
    Finalized,
}
