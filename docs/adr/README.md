# Architecture decision records

| # | Title | Status | Date |
| - | ----- | ------ | ---- |
| [0001](0001-collector-appliance-on-the-home-lan.md) | The collector is a dedicated appliance on the home LAN | Accepted | 2026-08-05 |
| [0002](0002-truenas-pulls-readings.md) | TrueNAS pulls readings; the collector holds no upstream credentials | Accepted | 2026-08-05 |
| [0003](0003-credentials-as-host-state.md) | Device credentials are one host-state file | Accepted | 2026-08-05 |
| [0004](0004-pull-deployment-pattern-reused.md) | Deployment reuses the ahara-vpn pull pattern | Accepted | 2026-08-05 |
| [0005](0005-dependency-free-service-with-vectored-crypto.md) | The service is dependency-free, including its KLAP crypto | Accepted | 2026-08-05 |
| [0006](0006-house-sensors-owns-the-data-schema.md) | house-sensors owns the data schema; the collector ships device-native readings | Accepted | 2026-08-06 |
| [0007](0007-per-module-reading-streams.md) | One reading stream per module; consumers never share a batch | Accepted | 2026-08-06 |
