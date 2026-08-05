# Partition layout for the S13's single NVMe disk: UEFI ESP plus an ext4
# root, no swap (12 GB RAM, zram is configured in hardware-configuration if
# ever needed). The disk path is a bootstrap flag — callers pass
# --argstr disk /dev/disk/by-id/... so no machine-specific device is
# committed.
{
  disk ? "/dev/disk/by-id/REPLACE_WITH_INSTALL_DISK",
  ...
}:
{
  disko.devices.disk.main = {
    type = "disk";
    device = disk;
    content = {
      type = "gpt";
      partitions = {
        esp = {
          size = "512M";
          type = "EF00";
          content = {
            type = "filesystem";
            format = "vfat";
            mountpoint = "/boot";
            mountOptions = [ "umask=0077" ];
          };
        };
        root = {
          size = "100%";
          content = {
            type = "filesystem";
            format = "ext4";
            mountpoint = "/";
          };
        };
      };
    };
  };
}
