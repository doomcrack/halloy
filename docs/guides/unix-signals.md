# Unix Signals

Frigicom supports Unix signals to control the application without restarting it.

## SIGUSR1

When Frigicom receives a `SIGUSR1` the application will reload the configuration
file.

### Example

```bash
pkill -u "$USER" --signal=SIGUSR1 ^frigicom$
```
