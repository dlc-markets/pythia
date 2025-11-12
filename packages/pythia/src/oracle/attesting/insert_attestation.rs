use crate::{
    data_models::oracle_msgs::Attestation, db::DBconnection, oracle::crypto::split_sig,
    oracle::error::Result,
};

impl DBconnection {
    /// Add signed outcome to meta data and digits signatures to DB and delete secret nonces to avoid secret key leaking
    pub(in crate::oracle) async fn update_to_attestation(
        &self,
        attestation: &Attestation,
        outcome: f64,
    ) -> Result<()> {
        let (indexes, sigs) = attestation
            .signatures
            .iter()
            .enumerate()
            .map(|(index, sig)| (index as i32, split_sig(*sig).1.to_be_bytes()))
            .collect::<(Vec<_>, Vec<_>)>();

        assert_eq!(
            attestation.outcomes.len(),
            sigs.len(),
            "outcomes and sigs must have the same length: inconsistent attestation"
        );

        // We use a transaction to rollback deleting secret nonces if there are more affected rows than expected:
        let mut tx = self.0.begin().await?;

        // SECURITY: secret nonce MUST be dropped from DB by setting all of them to null.
        // This ensures that a DB leakage would not immediately allow secret key extraction
        // Notice: secret key is still leaked if we sign events which secret nonce was in leaked DB
        let query_result = sqlx::query(
            "WITH events AS (
                UPDATE oracle.events SET outcome = $1::FLOAT8 WHERE id = $2::TEXT
            )
            UPDATE oracle.digits
        SET signature = bulk.sig, signing_ts = NOW(), nonce_secret = NULL
        FROM ( 
            SELECT *
            FROM UNNEST($3::BYTEA[], $4::INT[]) 
            AS t(sig, digit)
            ) AS bulk 
        WHERE event_id = $2 AND digit_index = bulk.digit AND nonce_secret IS NOT NULL
        ",
        )
        .bind(outcome)
        .bind(attestation.event_id)
        .bind(&sigs)
        .bind(indexes)
        .execute(&mut *tx)
        .await?;

        let affected_raws_counts = query_result.rows_affected();

        trace!("Rows affected count when updating to attestation: {affected_raws_counts}");

        if affected_raws_counts as usize != sigs.len() {
            match affected_raws_counts {
                0 => warn!(
                    "We tried to update an announcement into an attestation while it was already an attestation in postgres database. This is normal only if another pythia instance is running using the same database with the same private key."
                ),
                x @ 1.. => {
                    error!(
                        "Update of attestation does not affect the expected number of rows: {} instead of {}",
                        x,
                        sigs.len()
                    );
                }
            }

            warn!("Attestation insertion into postgres database has been cancelled");
            tx.rollback().await?;
        } else {
            tx.commit().await?;
        }

        Ok(())
    }
}
