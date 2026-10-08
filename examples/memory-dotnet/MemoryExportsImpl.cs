// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

using System;
using System.Collections.Generic;
using MemoryDotnetWorld.wit.Exports.microsoft.memoryDotnet;

namespace MemoryDotnetWorld.wit.Exports.microsoft.memoryDotnet;

public sealed class KnowledgeGraphOpsExportsImpl : IKnowledgeGraphOpsExports
{
    private static readonly List<IKnowledgeGraphOpsExports.Entity> Entities = new();
    private static readonly List<IKnowledgeGraphOpsExports.Relation> Relations = new();

    public static List<IKnowledgeGraphOpsExports.Entity> CreateEntities(
        List<IKnowledgeGraphOpsExports.Entity> entities)
    {
        var created = new List<IKnowledgeGraphOpsExports.Entity>();
        foreach (var entity in entities)
        {
            if (FindEntity(entity.name) is null)
            {
                Entities.Add(entity);
                created.Add(entity);
            }
        }

        return created;
    }

    public static List<IKnowledgeGraphOpsExports.Relation> CreateRelations(
        List<IKnowledgeGraphOpsExports.Relation> relations)
    {
        var created = new List<IKnowledgeGraphOpsExports.Relation>();
        foreach (var relation in relations)
        {
            if (!RelationExists(relation))
            {
                Relations.Add(relation);
                created.Add(relation);
            }
        }

        return created;
    }

    public static List<IKnowledgeGraphOpsExports.ObservationResult> AddObservations(
        List<IKnowledgeGraphOpsExports.ObservationInput> observations)
    {
        var results = new List<IKnowledgeGraphOpsExports.ObservationResult>();
        foreach (var input in observations)
        {
            var index = FindEntityIndex(input.entityName);
            if (index < 0)
            {
                throw new MemoryDotnetWorld.WitException<string>(
                    $"Entity with name {input.entityName} not found",
                    0);
            }

            var entity = Entities[index];
            var added = new List<string>();
            foreach (var content in input.contents)
            {
                if (!entity.observations.Contains(content))
                {
                    entity.observations.Add(content);
                    added.Add(content);
                }
            }

            Entities[index] = entity;
            results.Add(new IKnowledgeGraphOpsExports.ObservationResult(input.entityName, added));
        }

        return results;
    }

    public static void DeleteEntities(List<string> entityNames)
    {
        foreach (var name in entityNames)
        {
            Entities.RemoveAll(entity => entity.name == name);
            Relations.RemoveAll(relation => relation.fromEntity == name || relation.toEntity == name);
        }
    }

    public static void DeleteObservations(
        List<IKnowledgeGraphOpsExports.ObservationDeletion> deletions)
    {
        foreach (var deletion in deletions)
        {
            var index = FindEntityIndex(deletion.entityName);
            if (index < 0)
            {
                throw new MemoryDotnetWorld.WitException<string>(
                    $"Entity with name {deletion.entityName} not found",
                    0);
            }

            var entity = Entities[index];
            foreach (var observation in deletion.observations)
            {
                entity.observations.Remove(observation);
            }

            Entities[index] = entity;
        }
    }

    public static void DeleteRelations(List<IKnowledgeGraphOpsExports.Relation> relations)
    {
        foreach (var relation in relations)
        {
            Relations.RemoveAll(existing =>
                existing.fromEntity == relation.fromEntity &&
                existing.toEntity == relation.toEntity &&
                existing.relationType == relation.relationType);
        }
    }

    public static IKnowledgeGraphOpsExports.KnowledgeGraph ReadGraph()
    {
        return new IKnowledgeGraphOpsExports.KnowledgeGraph(
            new List<IKnowledgeGraphOpsExports.Entity>(Entities),
            new List<IKnowledgeGraphOpsExports.Relation>(Relations));
    }

    public static IKnowledgeGraphOpsExports.KnowledgeGraph SearchNodes(string query)
    {
        var normalized = query.ToLowerInvariant();
        var matchingNames = new HashSet<string>(StringComparer.Ordinal);
        var matchingEntities = new List<IKnowledgeGraphOpsExports.Entity>();
        foreach (var entity in Entities)
        {
            var matches = entity.name.Contains(normalized, StringComparison.OrdinalIgnoreCase) ||
                entity.entityType.Contains(normalized, StringComparison.OrdinalIgnoreCase);
            foreach (var observation in entity.observations)
            {
                matches |= observation.Contains(normalized, StringComparison.OrdinalIgnoreCase);
            }

            if (matches)
            {
                matchingNames.Add(entity.name);
                matchingEntities.Add(entity);
            }
        }

        return new IKnowledgeGraphOpsExports.KnowledgeGraph(
            matchingEntities,
            RelationsFor(matchingNames));
    }

    public static IKnowledgeGraphOpsExports.KnowledgeGraph OpenNodes(List<string> names)
    {
        var requested = new HashSet<string>(names, StringComparer.Ordinal);
        var entities = new List<IKnowledgeGraphOpsExports.Entity>();
        foreach (var entity in Entities)
        {
            if (requested.Contains(entity.name))
            {
                entities.Add(entity);
            }
        }

        return new IKnowledgeGraphOpsExports.KnowledgeGraph(entities, RelationsFor(requested));
    }

    private static IKnowledgeGraphOpsExports.Entity? FindEntity(string name)
    {
        var index = FindEntityIndex(name);
        return index < 0 ? null : Entities[index];
    }

    private static int FindEntityIndex(string name)
    {
        for (var index = 0; index < Entities.Count; index++)
        {
            if (Entities[index].name == name)
            {
                return index;
            }
        }

        return -1;
    }

    private static bool RelationExists(IKnowledgeGraphOpsExports.Relation relation)
    {
        foreach (var existing in Relations)
        {
            if (existing.fromEntity == relation.fromEntity &&
                existing.toEntity == relation.toEntity &&
                existing.relationType == relation.relationType)
            {
                return true;
            }
        }

        return false;
    }

    private static List<IKnowledgeGraphOpsExports.Relation> RelationsFor(
        HashSet<string> names)
    {
        var result = new List<IKnowledgeGraphOpsExports.Relation>();
        foreach (var relation in Relations)
        {
            if (names.Contains(relation.fromEntity) || names.Contains(relation.toEntity))
            {
                result.Add(relation);
            }
        }

        return result;
    }
}
