################################################################################
#
# onlyfansd
#
################################################################################

ONLYFANSD_VERSION = 0.1.0

# Point at the root of this repo (where Cargo.toml lives), relative to your
# Buildroot tree. Adjust if your layout differs; e.g. if onlyfans/ and
# buildroot/ are siblings:
#   ONLYFANSD_SITE = $(TOPDIR)/../onlyfans
ONLYFANSD_SITE = $(TOPDIR)/../onlyfans
ONLYFANSD_SITE_METHOD = local

ONLYFANSD_LICENSE = MIT
ONLYFANSD_INSTALL_TARGET = YES

define ONLYFANSD_INSTALL_INIT_SYSV
	$(INSTALL) -D -m 0755 $(ONLYFANSD_PKGDIR)/S99onlyfansd \
		$(TARGET_DIR)/etc/init.d/S99onlyfansd
endef

define ONLYFANSD_INSTALL_TARGET_CONF
	$(INSTALL) -d -m 0755 $(TARGET_DIR)/etc/onlyfansd
	$(INSTALL) -D -m 0600 $(ONLYFANSD_PKGDIR)/../../../config_sample.yaml \
		$(TARGET_DIR)/etc/onlyfansd/config.yaml.example
endef

ONLYFANSD_POST_INSTALL_TARGET_HOOKS += ONLYFANSD_INSTALL_TARGET_CONF

$(eval $(cargo-package))
