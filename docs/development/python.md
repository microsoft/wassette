# Authoring Wasm Components with Python

This guide walks you through creating WebAssembly (Wasm) components using Python that can be used as Tools for AI Agents with Wassette. Wassette allows you to run Python tools as secure, isolated Wasm components via the Model Context Protocol (MCP).

## Overview

Python Wasm components in Wassette:
- Are built using [componentize-py](https://github.com/bytecodealliance/componentize-py)
- Follow the [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
- Use [WIT (WebAssembly Interface Types)](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md) for interface definitions
- Run in a secure, sandboxed environment with policy-controlled capabilities

## Prerequisites

Before getting started, you'll need:

- Python 3.10 or higher
- [uv](https://docs.astral.sh/uv/) - A fast Python package manager and project manager
- Basic understanding of Python and WebAssembly concepts

### Installing uv

```bash
# Install uv if not already installed
curl -LsSf https://astral.sh/uv/install.sh | sh
```

## Project Structure

A typical Python Wasm component project has this structure:

```
my-component/
├── main.py              # Main Python implementation
├── pyproject.toml       # Python project configuration
├── Justfile            # Build automation (optional)
├── wit/                # WIT interface definitions
│   └── world.wit       # Component interface specification
├── wit_world/          # Generated Python bindings (auto-generated)
│   ├── __init__.py
│   └── types.py
└── README.md           # Component documentation
```

## Step-by-Step Tutorial

### Step 1: Create Project Structure

```bash
# Create a new directory for your component
mkdir my-python-tool
cd my-python-tool

# Create the necessary directories
mkdir wit wit_world
```

### Step 2: Define the WIT Interface

Create `wit/world.wit` to define your component's interface:

```wit
package local:my-tool;

/// Example world for a simple calculator tool
world calculator {
    /// Add two numbers and return the result
    export add: func(a: f64, b: f64) -> result<f64, string>;
    
    /// Perform a calculation from a string expression
    export calculate: func(expression: string) -> result<string, string>;
}
```

**WIT Interface Guidelines:**
- Use clear, descriptive function names
- Include documentation comments (`///`)
- Use `result<T, string>` for functions that can fail
- Choose appropriate data types (string, f64, s32, bool, etc.)
- Keep interfaces simple and focused

### Step 3: Configure Python Project

Create `pyproject.toml`:

```toml
[project]
name = "my-python-tool"
version = "0.1.0"
description = "A Python calculator tool"
readme = "README.md"
requires-python = ">=3.10"
dependencies = [
    "componentize-py>=0.17.1",
]
```

### Step 4: Generate Python Bindings

```bash
# Install componentize-py
uv pip install componentize-py

# Generate Python bindings from WIT
uv run componentize-py -d wit -w calculator bindings .
```

This creates the `wit_world/` directory with Python type definitions and interfaces.

### Step 5: Implement the Python Component

Create `main.py`:

```python
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

import wit_world
from wit_world.types import Err
import json

def handle_error(e: Exception) -> Err[str]:
    """Helper function to convert Python exceptions to WIT errors"""
    message = str(e)
    if message == "":
        return Err(f"{type(e).__name__}")
    else:
        return Err(f"{type(e).__name__}: {message}")

class Calculator(wit_world.Calculator):
    def add(self, a: float, b: float) -> float:
        """Add two numbers together"""
        try:
            result = a + b
            return result
        except Exception as e:
            raise handle_error(e)
    
    def calculate(self, expression: str) -> str:
        """Evaluate a mathematical expression and return JSON result"""
        try:
            # Simple evaluation - in production you'd want safer parsing
            result = eval(expression)
            return json.dumps({"result": result, "expression": expression})
        except Exception as e:
            raise handle_error(e)
```

**Implementation Best Practices:**
- Always handle exceptions and convert them to WIT errors
- Use descriptive class and method names
- Validate inputs before processing
- Return structured data as JSON strings when appropriate
- Keep functions focused and single-purpose

### Step 6: Build the Component

Create a `Justfile` for easy building:

```just
install-uv:
    if ! command -v uv &> /dev/null; then curl -LsSf https://astral.sh/uv/install.sh | sh; fi

install: install-uv
    uv pip install componentize-py

bindings:
    uv run componentize-py -d wit -w calculator bindings .

build:
    uv run componentize-py -d wit -w calculator componentize -s main -o calculator.wasm

all: bindings build
```

Build your component:

```bash
# Install build tools
just install

# Generate bindings and build Wasm component
just all

# Or run commands manually:
# uv run componentize-py -d wit -w calculator bindings .
# uv run componentize-py -d wit -w calculator componentize -s main -o calculator.wasm
```

### Step 7: Test Your Component

You can test your component using Wassette:

```bash
# Start Wassette with your component
wassette serve --sse --plugin-dir .

# Or load it explicitly
wassette load file://./calculator.wasm
```

## Advanced Topics

### Working with Complex Data Types

WIT supports rich data types. Here's how to work with them:

```wit
// In your world.wit file
world advanced-tool {
    // Records (structs)
    export process-user: func(user: user-info) -> result<string, string>;
    
    // Variants (enums/unions)
    export handle-event: func(event: app-event) -> result<string, string>;
    
    // Lists
    export process-batch: func(items: list<string>) -> result<list<string>, string>;
}

record user-info {
    name: string,
    age: u32,
    email: string,
}

variant app-event {
    user-login(user-info),
    user-logout(string),
    data-update(string),
}
```

```python
# In your Python implementation
from wit_world.types import UserInfo, AppEvent

class AdvancedTool(wit_world.AdvancedTool):
    def process_user(self, user: UserInfo) -> str:
        return json.dumps({
            "processed": True,
            "user": {
                "name": user.name,
                "age": user.age,
                "email": user.email
            }
        })
    
    def handle_event(self, event: AppEvent) -> str:
        if isinstance(event, AppEvent.UserLogin):
            return f"User {event.value.name} logged in"
        elif isinstance(event, AppEvent.UserLogout):
            return f"User {event.value} logged out"
        # ... handle other variants
    
    def process_batch(self, items: list[str]) -> list[str]:
        return [item.upper() for item in items]
```

### Error Handling Patterns

Use consistent error handling throughout your component:

```python
from wit_world.types import Err
import logging

class RobustTool(wit_world.RobustTool):
    def safe_operation(self, input_data: str) -> str:
        try:
            # Validate input
            if not input_data or not input_data.strip():
                raise ValueError("Input cannot be empty")
            
            # Process data
            result = self._process_data(input_data)
            
            # Return success
            return json.dumps({"success": True, "data": result})
            
        except ValueError as e:
            # Known validation errors
            raise Err(f"Validation error: {e}")
        except Exception as e:
            # Unexpected errors
            logging.error(f"Unexpected error: {e}")
            raise Err(f"Internal error: {type(e).__name__}")
    
    def _process_data(self, data: str) -> dict:
        # Your processing logic here
        return {"processed": data.upper()}
```

### Working with Resources and Imports

Some components may need to import functionality from the host:

```wit
world network-tool {
    // Import host capabilities (if allowed by policy)
    import wasi:http/outgoing-handler@0.2.0;
    
    // Export your functionality
    export fetch-data: func(url: string) -> result<string, string>;
}
```

```python
# Use imported capabilities in your implementation
# Note: This requires appropriate policy permissions
```

## Testing Your Component

### Unit Testing

Create tests for your component logic:

```python
# test_calculator.py
import unittest
from main import Calculator

class TestCalculator(unittest.TestCase):
    def setUp(self):
        self.calc = Calculator()
    
    def test_add(self):
        result = self.calc.add(2.0, 3.0)
        self.assertEqual(result, 5.0)
    
    def test_calculate(self):
        result = self.calc.calculate("2 + 3")
        self.assertIn("result", result)

if __name__ == '__main__':
    unittest.main()
```

### Integration Testing

Test your component with Wassette:

```bash
# Build your component
just build

# Start Wassette with your component
wassette serve --sse --plugin-dir .

# In another terminal, test with MCP inspector
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse
```

## Deployment and Distribution

### Building for Distribution

```bash
# Build optimized version
just build

# Your component is now available as calculator.wasm
```

### Packaging for OCI Registry

You can package and distribute your component via container registries:

```bash
# Package your component (requires additional tooling)
# This is typically done via CI/CD pipelines
```

## Common Patterns and Examples

### File Processing Tool

```wit
world file-processor {
    export process-text: func(content: string, operation: string) -> result<string, string>;
    export get-file-info: func(filename: string) -> result<string, string>;
}
```

```python
import json
import re
from pathlib import Path

class FileProcessor(wit_world.FileProcessor):
    def process_text(self, content: str, operation: str) -> str:
        operations = {
            "uppercase": lambda x: x.upper(),
            "lowercase": lambda x: x.lower(),
            "word_count": lambda x: str(len(x.split())),
            "line_count": lambda x: str(len(x.splitlines())),
        }
        
        if operation not in operations:
            raise Err(f"Unknown operation: {operation}")
        
        result = operations[operation](content)
        return json.dumps({"operation": operation, "result": result})
    
    def get_file_info(self, filename: str) -> str:
        try:
            # Note: File access requires appropriate policy permissions
            path = Path(filename)
            if not path.exists():
                raise Err(f"File not found: {filename}")
            
            return json.dumps({
                "name": path.name,
                "size": path.stat().st_size,
                "extension": path.suffix,
                "is_file": path.is_file(),
            })
        except Exception as e:
            raise handle_error(e)
```

### Data Analysis Tool

```wit
world data-analyzer {
    export analyze-csv: func(csv-data: string) -> result<string, string>;
    export calculate-stats: func(numbers: list<f64>) -> result<string, string>;
}
```

```python
import json
import csv
import io
from statistics import mean, median, stdev

class DataAnalyzer(wit_world.DataAnalyzer):
    def analyze_csv(self, csv_data: str) -> str:
        try:
            reader = csv.DictReader(io.StringIO(csv_data))
            rows = list(reader)
            
            return json.dumps({
                "row_count": len(rows),
                "columns": list(rows[0].keys()) if rows else [],
                "sample_row": rows[0] if rows else None,
            })
        except Exception as e:
            raise handle_error(e)
    
    def calculate_stats(self, numbers: list[float]) -> str:
        try:
            if not numbers:
                raise ValueError("Cannot calculate stats for empty list")
            
            return json.dumps({
                "count": len(numbers),
                "mean": mean(numbers),
                "median": median(numbers),
                "std_dev": stdev(numbers) if len(numbers) > 1 else 0,
                "min": min(numbers),
                "max": max(numbers),
            })
        except Exception as e:
            raise handle_error(e)
```



## Resources

- [ComponentModel Documentation](https://component-model.bytecodealliance.org/language-support/python.html)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [componentize-py Repository](https://github.com/bytecodealliance/componentize-py)
- [Wassette Examples](../../examples/)
- [MCP Protocol Documentation](https://github.com/modelcontextprotocol/specification)

## Next Steps

1. Review the [eval-py example](../../examples/eval-py/) for a working implementation
2. Experiment with different WIT interface designs
3. Explore policy configurations for different use cases
4. Consider contributing your component to the Wassette examples

---

*For more information about Wassette and the Model Context Protocol, see the [main documentation](../overview.md).*