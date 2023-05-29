use tokio_postgres::{types::Type, Client, Statement};

const QUERY_INSERT_ANNOUNCEMENT: &str = "INSERT INTO oracle.events VALUES ($1, $2, $3, $4, $5)";
// id, digits, precision, maturity, announcement_signature)";

const QUERY_INSERT_ATTESTATION: &str = "INSERT INTO oracle.digits VALUES ($1, $2, $3, $4)"; // (event_id, digit_index, nonce_public, nonce_secret)"

const QUERY_UPDATE_ANNOUNCEMENT: &str = "UPDATE oracle.events
SET outcome = $1
WHERE id = $2;
";

const QUERY_UPDATE_ATTESTATION: &str = "UPDATE oracle.digits
SET 
  bit = $1,
  signature = $2,
  signing_ts = NOW(),
  nonce_secret = NULL
WHERE 
  event_id = $3
  AND digit_index = $4;
";

const QUERY_GET_ORACLE_EVENT_INFO: &str = "SELECT
e.*
FROM oracle.events e
WHERE e.id = $1";

const QUERY_GET_ORACLE_EVENT: &str = "SELECT 
d.*
FROM oracle.digits d
WHERE d.event_id = $1;
";

pub struct DatabaseConnection {
    pub client: Client,
    pub insert_announcement_statement: Statement,
    pub insert_attestation_statement: Statement,
    pub update_announcement_statement: Statement,
    pub update_attestation_statement: Statement,
    pub get_oracle_event_info_statement: Statement,
    pub get_oracle_event_statement: Statement,
}

impl DatabaseConnection {
    pub async fn new(postgres_connection_string: &str) -> Result<Self, tokio_postgres::Error> {
        // Connect to the database.
        let (client, connection) =
            tokio_postgres::connect(postgres_connection_string, tokio_postgres::NoTls).await?;

        // The connection object performs the actual communication with the database,
        // so spawn it off to run on its own.
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        let insert_announcement_statement = client
            .prepare_typed(
                QUERY_INSERT_ANNOUNCEMENT,
                &[
                    Type::VARCHAR,
                    Type::OID,
                    Type::INT4,
                    Type::TIMESTAMP,
                    Type::BIT,
                ],
            )
            .await?;
        let insert_attestation_statement = client
            .prepare_typed(
                QUERY_INSERT_ATTESTATION,
                &[Type::VARCHAR, Type::OID, Type::BIT, Type::BIT],
            )
            .await?;
        let update_announcement_statement = client
            .prepare_typed(QUERY_UPDATE_ANNOUNCEMENT, &[Type::OID, Type::VARCHAR])
            .await?;
        let update_attestation_statement = client
            .prepare_typed(
                QUERY_UPDATE_ATTESTATION,
                &[Type::BIT, Type::BIT, Type::VARCHAR, Type::OID],
            )
            .await?;
        let get_oracle_event_info_statement = client
            .prepare_typed(QUERY_GET_ORACLE_EVENT_INFO, &[Type::VARCHAR])
            .await?;
        let get_oracle_event_statement = client
            .prepare_typed(QUERY_GET_ORACLE_EVENT, &[Type::VARCHAR])
            .await?;

        Ok(Self {
            client,
            insert_announcement_statement,
            insert_attestation_statement,
            update_announcement_statement,
            update_attestation_statement,
            get_oracle_event_info_statement,
            get_oracle_event_statement,
        })
    }
    pub async fn is_empty(&self) -> bool {
        self.client
            .query("SELECT * FROM oracle.events LIMIT 1", &[])
            .await
            .unwrap()
            .is_empty()
    }
}
#[cfg(test)]
mod test {
    use crate::oracle::postgres::DatabaseConnection;
    #[tokio::test]
    async fn postgres_connection_check() {
        let client = DatabaseConnection::new("host=localhost user=postgres password=postgres")
            .await
            .unwrap();
        // Now we can execute a simple statement that just returns its parameter.
        let rows = client
            .client
            .query("SELECT $1::TEXT", &[&"hello world"])
            .await
            .unwrap();

        // And then check that we got back the same string we sent over.
        let value: &str = rows[0].get(0);
        assert_eq!(value, "hello world");
    }
}
