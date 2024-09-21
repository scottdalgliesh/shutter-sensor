# Shutter-Sensor

Firmware for a wireless hall-effect sensor to check if your window is shut(ter).

## Description
* Powered by [embassy](https://github.com/embassy-rs/embassy), running on the ESP32C3 microcontroller.
* Intended for use with [shutter](https://github.com/scottdalgliesh/shutter) to displays live sensor status via web browser.

## Set-up
* Wire ESP32C3 and hall effect sensor per the [wiring schematic](schematic\shutter_sensor.pdf).
* Start [shutter](https://github.com/scottdalgliesh/shutter) server to accept incoming sensor status. Note server URL.
* Clone this repo, and add a '.env' file to the project root with the following environment variables to match your wifi credentials (SSID & PASSWORD) and [shutter](https://github.com/scottdalgliesh/shutter) server URL (URL):
    ```
    SSID="XXXXX"
    PASSWORD="XXXXX"
    URL="XXX.XXX.XX.XX:XXXX"
    ```
* Flash and run firmware via USB using `cargo run` in your console (requires [Rust](https://www.rust-lang.org/learn/get-started) toolchain installed on your system). 
* Navigate to the URL noted above in your web browser to view live sensor status.
* Depending on your network settings, it may be necessary to configure your firewall to accept traffic to the specific port your server is using. See instructions in [shutter](https://github.com/scottdalgliesh/shutter) repo as required.

## License

[MIT](https://choosealicense.com/licenses/mit/)