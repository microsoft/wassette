# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

from typing import TypeVar, Generic, Union, Optional, Protocol, Tuple, List, Any, Self
from types import TracebackType
from enum import Flag, Enum, auto
from dataclasses import dataclass
from abc import abstractmethod
import weakref

from .types import Result, Ok, Err, Some



class WitWorld(Protocol):

    @abstractmethod
    def eval(self, expression: str) -> str:
        """
        Raises: `wit_world.types.Err(wit_world.imports.str)`
        """
        raise NotImplementedError

    @abstractmethod
    def exec(self, statements: str) -> str:
        """
        Raises: `wit_world.types.Err(wit_world.imports.str)`
        """
        raise NotImplementedError

