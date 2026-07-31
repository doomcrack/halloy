# Portable Mode

To enable portable mode, place the `config.toml` file in the same directory as
the running executable.

```
.
├── frigicom
└── config.toml
```

Or set the `FRIGICOM_PORTABLE_DIR` environment variable to a valid directory
path explicitly. Both config and data (including the private
[logoscore instance directory](../configuration/logos.md#instance_dir)) then
live in that directory.
