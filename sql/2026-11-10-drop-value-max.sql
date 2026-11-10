-- `value_max` was written by the rollup but never read: the series query uses
-- the per-bucket average (and the highest replica's average). Drop it.
ALTER TABLE `dash_point_hourly` DROP COLUMN `value_max`;
