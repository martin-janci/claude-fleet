//! What a host may take from a catalog it does not own: the acceptance
//! rule (M2). Task 3 extends this file with the rest of the "effective
//! catalog for a host" computation.

/// What a host may take from a catalog, having decided which of the
/// catalog's assets it would otherwise want.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acceptance {
    /// Nothing from this catalog.
    No,
    /// Only assets marked `scope: shared`.
    SharedOnly,
    /// Every asset.
    All,
}

/// What a host with `host_org` may take from a catalog owned by
/// `catalog_org` (`None` = personal), given the host's admissions (M3;
/// always empty in M2).
///
/// - personal catalog: a host with no org gets `All`; a host with an org
///   gets `SharedOnly`.
/// - org catalog `X`: a host in org `X` gets `All`; a host with no org
///   whose admissions include `catalog_id` gets `All`; everyone else gets
///   `No`. Admission never crosses orgs — a host in a *different* org gets
///   `No` even if `catalog_id` is in its admissions list.
pub fn acceptance(
    host_org: Option<i64>,
    catalog_id: i64,
    catalog_org: Option<i64>,
    admitted: &[i64],
) -> Acceptance {
    match (catalog_org, host_org) {
        (None, None) => Acceptance::All,
        (None, Some(_)) => Acceptance::SharedOnly,
        (Some(c), Some(h)) if c == h => Acceptance::All,
        (Some(_), None) if admitted.contains(&catalog_id) => Acceptance::All,
        _ => Acceptance::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PERSONAL: i64 = 1;
    const ACME: i64 = 2;
    const ACME_ORG: i64 = 10;

    #[test]
    fn a_host_without_an_org_takes_all_of_personal_and_only_admitted_org_catalogs() {
        assert_eq!(acceptance(None, PERSONAL, None, &[]), Acceptance::All);
        assert_eq!(acceptance(None, ACME, Some(ACME_ORG), &[]), Acceptance::No);
        assert_eq!(
            acceptance(None, ACME, Some(ACME_ORG), &[ACME]),
            Acceptance::All
        );
    }

    #[test]
    fn an_org_host_takes_its_org_catalog_and_only_shared_personal_assets() {
        assert_eq!(
            acceptance(Some(ACME_ORG), PERSONAL, None, &[]),
            Acceptance::SharedOnly
        );
        assert_eq!(
            acceptance(Some(ACME_ORG), ACME, Some(ACME_ORG), &[]),
            Acceptance::All
        );
        assert_eq!(
            acceptance(Some(99), ACME, Some(ACME_ORG), &[ACME]),
            Acceptance::No,
            "admission never crosses orgs"
        );
    }
}
