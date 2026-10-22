.PHONY: deb deb-arm64 deb-armhf

deb: deb-arm64 deb-armhf

deb-arm64:
	devenv shell -- deb arm64

deb-armhf:
	devenv shell -- deb armhf
