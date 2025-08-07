---
applyTo: "examples/**/*.js"
---

Wassette is a security-oriented runtime that runs WebAssembly (Wasm) components via Model Context Protocol (MCP). Wasm Components can be compiled from various languages, including JavaScript.

# JavaScript-based Wasm Component Code Standards

JavaScript-compiles WebAssembly code is a bit different from the traditional JavaScript code. It needs to work with a few tools:

- `StarlingMonkey` a WebAssembly component aware Javascript engine
- `componentize-js` a tool for building WebAssembly components from Javascript files
- `jco` a multi-tool for componentizing, type generation, and running components in NodeJS and browser contexts

## Check Tools

You should make sure that you have the following tools installed:

``` 
jco --version
wasmtime --version
wasm-tools --version
npm --version
wkg --version
```

## Overview of building a component with JavaScript

1. Author the WebAssembly Interface Types (WIT) files in `wit` directory that define the component's interface.
2. Write the JavaScript code that satisfies the interface.
3. Compile the interface-compliant JavaScript code into a WebAssembly component using `jco`.

### What is WIT?

WebAssembly Interface Types ("WIT") is a featureful Interface Definition Language ("IDL") for defining functionality. Examples:

```wit
package docs:adder@0.1.0;

interface add {
    add: func(x: u32, y: u32) -> u32;
}

world adder {
    export add;
}
```

or,

```wit
package test:mcp@0.1.0;

world weather-mcp {
    import wasi:cli/environment@0.2.0;
    export get-weather: func(city: string) -> string;
}
```

You can learn more about WIT syntax and features in the [WIT documentation](https://component-model.bytecodealliance.org/design/wit.html)

To implement the `adder` world, we can write a JavaScript ES module:

```javascript
export const add = {
    add(x, y) {
        return x + y;
    }
};
```

When building your JavaScript project, ensure to set the "type":"module" option in package.json, as `jco` works exclusively with JavaScript modules.

In the code above:

- The adder world is analogous to the JavaScript module (file) itself
- The exported add object mirrors the exported add interface in WIT
- The add function mirrors the add function inside the add interface
- With the WIT and JavaScript in place, we can use jco to create a WebAssembly component from the JS module, using jco componentize.

This component does not use any of the WebAssembly System Interface (WASI) features, so to build a component, we can run:

```bash
jco componentize \
    --wit path/to/adder/world.wit \
    --world-name example \
    --out adder.wasm \
    --disable all \
    path/to/adder.js
```

## Running the component


Run the following command to run the component:

```bash
wasmtime run -Shttp --invoke 'add(3,2)' adder.wasm
```

This will invoke the `add` function from the `adder` component with the arguments `3` and `2`, and it should return `5`.

## Additional examples

### Using WASI

Consider a component taht exports a function to get weather information for a given city. The WIT file might look like this:

```wit
package mossaka:mcp@0.1.0;

world weather-mcp {
    import wasi:cli/environment@0.2.0;
    export get-weather: func(city: string) -> string;
}
```

This WIT package imports the `wasi:cli/environment` package, which provides access to the WebAssembly System Interface (WASI) features. You should place this WIT file in a directory like `wit/weather-mcp.wit`.

Now, you need a tool named `wkg` to fetch the dependencies for the WIT file. You can install it using:

And then run the following command to fetch the dependencies:

```bash
wkg wit fetch
```

This will download the necessary WIT files and place them in a `wit/deps` directory. 

IMPORTANT: If you run into issues with `wkg wit fetch`, you may copy and paste the WIT files from `wit-template` directory to the `wit` directory. The `wit-template` directory contains the necessary WIT files for the `wasi:cli/environment` and `wasi:http/proxy` packages. The convention for WIT package dependencies is to place them in a `wit/deps` directory, so when you copy the WIT files, ensure that the directory structure is maintained.

Next, you can implement the `weather-mcp` world in JavaScript. Create a file named `weather-mcp.js`:

```javascript
import { getEnvironment } from "wasi:cli/environment@0.2.0";  

export async function getWeather(city) {
  try {
    const env = getEnvironment();
    const apiKey = env.find(([key]) => key === 'OPENWEATHER_API_KEY')?.[1];
    if (apiKey === undefined) {
      return "Error: OPENWEATHER_API_KEY is not set";
    }
    const geoResponse = await fetch(
      `https://api.openweathermap.org/geo/1.0/direct?q=${city}&limit=1&appid=${apiKey}`
    );
    if (!geoResponse.ok) {
      return "Error: Failed to fetch geo data";
    }
    const geoData = await geoResponse.json();
    const lat = geoData[0].lat;
    const lon = geoData[0].lon;

    const response = await fetch(
      `https://api.openweathermap.org/data/2.5/weather?lat=${lat}&lon=${lon}&appid=${apiKey}&units=metric`
    )
    if (!response.ok) {
      return "Error: Failed to fetch weather data";
    }
    const data = await response.json();
    const weather = data.main.temp.toString();
    return weather;
  } catch (error) {
    return "Error fetching weather data";
  }
}
```

This JavaScript code implements the `getWeather` function, which fetches the weather data for a given city using the OpenWeatherMap API. It retrieves the API key from the WASI store and uses it to make requests to the OpenWeatherMap API.

Finally, your `package.json` should look like this:

```json
{
    "type": "module",
    "dependencies": {
        "@bytecodealliance/componentize-js": "0.18.1",
        "@bytecodealliance/jco": "^1.11.1"
    },
    "scripts": {
        "build:component": "jco componentize -w ./wit weather-mcp.js -d stdio -d random -d clocks -o weather-mcp.wasm",
    },
    "license": "Apache-2.0"
}
```

This configuration sets the type to "module" and includes the necessary dependencies for building the component. The `build:component` script uses `jco` to create the WebAssembly component from the JavaScript file and WIT definitions. Note that the `-d` flags are used to disable specific WASI features, such as standard input/output, random number generation, and clocks.

You can then build the component by running:

```bash
npm install
npm run build:component
``` 

This will generate the `weather-mcp.wasm` file.

To test this in Wasmtime, you can run the following command:

```bash
wasmtime run --env OPENWEATHER_API_KEY=<your_api_key> -Shttp --invoke 'get-weather("London")' weather-mcp.wasm
```

If your environment does not have the `OPENWEATHER_API_KEY` set, you should test it writing a script to mock the API responses.

### Time Component

Consider a component that exports a function to get the current time. The WIT file might look like this:

```wit
package local:time-server;

interface time {
    get-current-time: func() -> string;
}

world time-server {
    export time;
} 
```

You can implement this in JavaScript as follows:

```javascript
async function getCurrentTime() {
    return new Date().toISOString();
}

export const time = {
    getCurrentTime
};
```

You can then write a `package.json` file like this:

```json
{
  "name": "time-server",
  "version": "1.0.0",
  "main": "time.js",
  "scripts": {
    "build": "jco componentize ./time.js --wit ./wit -d http -d random -d stdio -o ./time.wasm"
  },
  "license": "Apache-2.0",
  "dependencies": {
        "@bytecodealliance/componentize-js": "0.18.1",
        "@bytecodealliance/jco": "^1.11.1"
  }
} 
```

## How to Debug

You can run the transpile command to see the generated JavaScript code (assuming that the file name is `weather.js`):

```bash
npm run transpile                       

> transpile
> jco transpile weather.wasm -o out-dir


  Transpiled JS Component Files:

 - out-dir/interfaces/wasi-config-store.d.ts           0.35 KiB
 - out-dir/interfaces/wasi-http-outgoing-handler.d.ts  0.47 KiB
 - out-dir/interfaces/wasi-http-types.d.ts             9.02 KiB
 - out-dir/interfaces/wasi-io-error.d.ts               0.15 KiB
 - out-dir/interfaces/wasi-io-poll.d.ts                0.23 KiB
 - out-dir/interfaces/wasi-io-streams.d.ts             0.82 KiB
 - out-dir/weather.core.wasm                           10.6 MiB
 - out-dir/weather.d.ts                                0.73 KiB
 - out-dir/weather.js     
 ```

## Perform a `fetch` request

The following example demonstrates how to perform a `fetch` request in a WebAssembly component using JavaScript:

```javascript
import { argv } from "node:process";
import { URL } from "node:url";

import { simpleRequest } from "./dist/transpiled/component.js";

const DEFAULT_URL = "https://jsonplaceholder.typicode.com/posts/1";

async function main() {
  // NOTE: argv should look like [path/to/node, path/to/script, ...args]
  let requestURL = DEFAULT_URL;
  if (argv[2]) {
    try {
      console.log(`parsing URL: [${argv[2]}]...`);
      requestURL = new URL(argv[1]).toString();
    } catch (err) {
      console.log(`ERROR: failed to build URL from argument [${argv[1]}]`);
      throw err;
    }
  }

  // NOTE: in the case of a missing/undefined requestURL, the component will use the default
  const { url, responseJson } = simpleRequest.getJson(requestURL);
  console.log(`Performed HTTP GET request [${url}]`);
  console.log({
    url,
    responseJson: JSON.parse(responseJson),
  });
}

await main();
```

with the following WIT file:

```wit
package example:node-fetch;

interface simple-request {
  record response {
    /// User-provided name for a request
    url: string,

    /// Response body, converted to JSON
    response-json: string,
  }

  /// This function performs a GET request
  /// to the provided URL without supplying an query parameters or a body
  get-json: func(url: option<string>) -> response;
}

world component {
 export simple-request;
}
```

and the following `package.json`:

```json
{
  "name": "node-fetch",
  "description": "Example of using fetch API from a WebAssembly component with jco",
  "type": "module",
  "scripts": {
    "build:component": "jco componentize -w wit -o component.wasm component.js ",
    "transpile": "jco transpile -o dist/transpiled component.wasm",
    "demo": "node demo.js",
    "all": "npm run build:component && npm run transpile && npm run demo"
  },
  "dependencies": {
    "@bytecodealliance/jco": "^1.11.1",
    "@bytecodealliance/componentize-js": "^0.18.3"
  }
}
```

## Get Environment Variables

The current toolchain does not support standard nodejs way of getting environment variables, i.e. `process.env`. Instead, you can use the `wasi:cli/environment` package to get environment variables in a WebAssembly component.

In WIT, you can define the environment interface as follows:

```wit
package local:slack-webhook@0.1.0;

world slack-webhook {
    import wasi:cli/environment@0.2.0;
    export post-to-slack: func(message: string) -> bool;
}
```

And then you need to run `wkg wit fetch` to fetch the dependencies, assuming the WIT file is located in the `wit` directory (or you can copy the WIT file from the `wit-template` directory to the `wit` directory).

Next, you can import the `getEnvironment` function from the `wasi:cli/environment` package in your JavaScript code:

```javascript
import { getEnvironment } from "wasi:cli/environment@0.2.0";  

const env = getEnvironment();
const webhookUrl = env.find(([key]) => key === 'SLACK_WEBHOOK_URL')?.[1];
```

The `getEnvironment` function returns an array of key-value array representing the environment variables. You can then use this to access the environment variables in your component.

## Troubleshooting

### Issues with `wasmtime` commands

If you encounter issues with `wasmtime` commands, ensure that you have the correct version installed. The minimum required version is 33.0.0. You can check your version by running:

```bash
wasmtime run --help

Runs a WebAssembly module

Usage: wasmtime run [OPTIONS] <WASM>...

Arguments:
  <WASM>...
          The WebAssembly module to run and arguments to pass to it.
          
          Arguments passed to the wasm module will be configured as WASI CLI
          arguments unless the `--invoke` CLI argument is passed in which case
          arguments will be interpreted as arguments to the function specified.

Options:
  -O, --optimize <KEY[=VAL[,..]]>
          Optimization and tuning related options for wasm performance, `-O
          help` to see all

  -C, --codegen <KEY[=VAL[,..]]>
          Codegen-related configuration options, `-C help` to see all

  -D, --debug <KEY[=VAL[,..]]>
          Debug-related configuration options, `-D help` to see all

  -W, --wasm <KEY[=VAL[,..]]>
          Options for configuring semantic execution of WebAssembly, `-W help`
          to see all

  -S, --wasi <KEY[=VAL[,..]]>
          Options for configuring WASI and its proposals, `-S help` to see all

      --target <TARGET>
          The target triple; default is the host triple

      --config <FILE>
          Use the specified TOML configuration file. This TOML configuration
          file can provide same configuration options as the `--optimize`,
          `--codgen`, `--debug`, `--wasm`, `--wasi` CLI options, with a couple
          exceptions.
          
          Additional options specified on the command line will take precedent
          over options loaded from this TOML file.

      --allow-precompiled
          Allow executing precompiled WebAssembly modules as `*.cwasm` files.
          
          Note that this option is not safe to pass if the module being passed
          in is arbitrary user input. Only `wasmtime`-precompiled modules
          generated via the `wasmtime compile` command or equivalent should be
          passed as an argument with this option specified.

      --profile <STRATEGY>
          Profiling strategy (valid options are: perfmap, jitdump, vtune, guest)
          
          The perfmap, jitdump, and vtune profiling strategies integrate
          Wasmtime with external profilers such as `perf`. The guest profiling
          strategy enables in-process sampling and will write the captured
          profile to `wasmtime-guest-profile.json` by default which can be
          viewed at https://profiler.firefox.com/.
          
          The `guest` option can be additionally configured as:
          
          --profile=guest[,path[,interval]]
          
          where `path` is where to write the profile and `interval` is the
          duration between samples. When used with `--wasm-timeout` the timeout
          will be rounded up to the nearest multiple of this interval.

      --dir <HOST_DIR[::GUEST_DIR]>
          Grant access of a host directory to a guest.
          
          If specified as just `HOST_DIR` then the same directory name on the
          host is made available within the guest. If specified as `HOST::GUEST`
          then the `HOST` directory is opened and made available as the name
          `GUEST` in the guest.

      --env <NAME[=VAL]>
          Pass an environment variable to the program.
          
          The `--env FOO=BAR` form will set the environment variable named `FOO`
          to the value `BAR` for the guest program using WASI. The `--env FOO`
          form will set the environment variable named `FOO` to the same value
          it has in the calling process for the guest, or in other words it will
          cause the environment variable `FOO` to be inherited.

      --invoke <FUNCTION>
          The name of the function to run

      --preload <NAME=MODULE_PATH>
          Load the given WebAssembly module before the main module

      --argv0 <ARGV0>
          Override the value of `argv[0]`, typically the name of the executable
          of the application being run.
          
          This can be useful to pass in situations where a CLI tool is being
          executed that dispatches its functionality on the value of `argv[0]`
          without needing to rename the original wasm binary.

  -h, --help
          Print help (see a summary with '-h')
```

The `--invoke` option is used to specify the function to invoke within the WebAssembly component. For example, the adder component can be run with:

```bash
wasmtime run -Shttp --invoke 'add(3,2)' adder.wasm
```

The `-Shttp` option is used to enable the HTTP server. To understand that the component imports the `wasi:http` package, you can check the WIT file for the component by running:

```bash
wasm-tools component wit adder.wasm
```

The following is the encoding of WebAssembly values that are passed to the --invoke option:
```
Type	Example Values
Bools	true, false
Integers	123, -9
Floats	3.14, 6.022e+23, nan, -inf
Chars	'x', '☃︎', '\'', '\u{0}'
Strings	"abc\t123"
Tuples	("abc", 123)
Lists	[1, 2, 3]
Records	{field-a: 1, field-b: "two"}
Variants	days(30), forever
Enums	south, west
Options	"flat some", some("explicit some"), none
Results	"flat ok", ok("explicit ok"), err("oops")
Flags	{read, write}, {}
```

### Troubleshooting with `wasm-tools` command

wasm-tools can help you inspect the WebAssembly component and its WIT definitions. For example, you can run:

```bash
wasm-tools component wit weather-mcp.wasm
```

This will display the WIT definitions for the `weather-mcp` component, allowing you to verify that the WIT file is correctly defined and that the component is built as expected.

There are some useful commands you can run with `wasm-tools`:

```bash
# Print the WIT interface of a component
$ wasm-tools component wit component.wasm

# Convert WIT text files to a binary-encoded WIT package, printing the result to
# stdout
$ wasm-tools component wit ./wit -t

# Convert a WIT document to JSON
$ wasm-tools component wit ./wit --json

# Round trip WIT through the binary-encoded format to stdout.
$ wasm-tools component wit ./wit --wasm | wasm-tools component wit
```

The wasm-tools binary internally contains a number of subcommands for working with wasm modules and component. Many subcommands also come with Rust crates that can be use programmatically as well:

CLI	Rust Crate	Playground	Description

- `wasm-tools validate`	wasmparser		Validate a WebAssembly file
- `wasm-tools parse`	wat and wast	parse	Translate the WebAssembly text format to binary
- `wasm-tools print`	wasmprinter	print	Translate the WebAssembly binary format to text
- `wasm-tools smith`	wasm-smith		Generate a valid WebAssembly module from an input seed
- `wasm-tools mutate`	wasm-mutate		Mutate an input wasm file into a new valid wasm file
- `wasm-tools shrink`	wasm-shrink		Shrink a wasm file while preserving a predicate
- `wasm-tools dump`			Print debugging information about the binary format
- `wasm-tools objdump`			Print debugging information about section headers
- `wasm-tools strip`			Remove custom sections from a WebAssembly file
- `wasm-tools demangle`			Demangle Rust and C++ symbol names in the name section
- `wasm-tools compose`	wasm-compose		Compose wasm components together (deprecated)
- `wasm-tools component new`	wit-component		Create a component from a core wasm binary
- `wasm-tools component wit`			Extract a *.wit interface from a component
- `wasm-tools component embed`			Embed a component-type custom section in a core wasm binary
- `wasm-tools component unbundle`			Extract core wasm modules from a component
- `wasm-tools metadata show`	wasm-metadata		Show name and producer metadata in a component or module
- `wasm-tools metadata add`			Add name or producer metadata to a component or module
- `wasm-tools addr2line`			Translate wasm offsets to filename/line numbers with DWARF
- `wasm-tools completion`			Generate shell completion scripts for wasm-tools
- `wasm-tools json-from-wast`			Convert a *.wast file into JSON commands
- `wasm-tools wast`			Validate the structure of a *.wast file

The wasm-tools CLI contains useful tools for debugging WebAssembly modules and components. The various subcommands all have --help explainer texts to describe more about their functionality as well.

## Additional Resources

You may find more information about the `jco` tool in the [jco documentation](https://bytecodealliance.github.io/jco/).