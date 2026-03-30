# Product Identity

Your product has a unique identity, assigned by the Store. If you build your package manually, you'll need to include its identity details. (If you're using Visual Studio, this is done automatically.) [Learn more](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/app-identity)

## Package Manifest Identity

Include these values in your package manifest:

| Manifest Path | Value |
| --- | --- |
| `Package/Identity/Name` | `AuraXLabs.AuraTerm` |
| `Package/Identity/Publisher` | `CN=671C654E-E6B4-48F6-9D75-058B100AA46A` |
| `Package/Properties/PublisherDisplayName` | `Aura X Labs` |

Together, these elements declare the identity of your app, establishing the "package family" to which all of its packages belong. Individual packages will have additional details, such as architecture and version.

## Package Family

The package family can also be expressed in calculated forms which are not declared in the manifest:

| Property | Value |
| --- | --- |
| Package Family Name (PFN) | `AuraXLabs.AuraTerm_g018z8wqa1vdm` |

## Store Listing

You can share the direct link and Store ID to help customers find your app in the Store:

| Property | Value |
| --- | --- |
| URL | https://apps.microsoft.com/detail/9P6B6G5QGGWT |
| Store ID | `9P6B6G5QGGWT` |
| Store protocol link | `ms-windows-store://pdp/?productid=9P6B6G5QGGWT` |
