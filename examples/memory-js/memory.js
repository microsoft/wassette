// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

// In-memory storage for the knowledge graph
// This will be persisted across calls within the same session
let entities = [];
let relations = [];

/**
 * Helper function to find an entity by name
 */
function findEntity(name) {
    return entities.find(e => e.name === name);
}

/**
 * Helper function to check if a relation already exists
 */
function relationExists(fromEntity, toEntity, relationType) {
    return relations.some(r => 
        r.fromEntity === fromEntity && 
        r.toEntity === toEntity && 
        r.relationType === relationType
    );
}

/**
 * Create new entities in the knowledge graph
 */
function createEntities(newEntities) {
    try {
        const created = [];
        
        for (const entity of newEntities) {
            // Check if entity already exists
            if (!findEntity(entity.name)) {
                const newEntity = {
                    name: entity.name,
                    entityType: entity.entityType,
                    observations: entity.observations || []
                };
                entities.push(newEntity);
                created.push(newEntity);
            }
        }
        
        return { tag: "ok", val: created };
    } catch (error) {
        return { tag: "err", val: `Failed to create entities: ${error.message}` };
    }
}

/**
 * Create new relations between entities
 */
function createRelations(newRelations) {
    try {
        const created = [];
        
        for (const relation of newRelations) {
            // Check if relation already exists
            if (!relationExists(relation.fromEntity, relation.toEntity, relation.relationType)) {
                const newRelation = {
                    fromEntity: relation.fromEntity,
                    toEntity: relation.toEntity,
                    relationType: relation.relationType
                };
                relations.push(newRelation);
                created.push(newRelation);
            }
        }
        
        return { tag: "ok", val: created };
    } catch (error) {
        return { tag: "err", val: `Failed to create relations: ${error.message}` };
    }
}

/**
 * Add observations to existing entities
 */
function addObservations(observationInputs) {
    try {
        const results = [];
        
        for (const input of observationInputs) {
            const entity = findEntity(input.entityName);
            
            if (!entity) {
                return { 
                    tag: "err", 
                    val: `Entity with name ${input.entityName} not found` 
                };
            }
            
            const addedObservations = [];
            
            for (const content of input.contents) {
                // Only add if observation doesn't already exist
                if (!entity.observations.includes(content)) {
                    entity.observations.push(content);
                    addedObservations.push(content);
                }
            }
            
            results.push({
                entityName: input.entityName,
                addedObservations: addedObservations
            });
        }
        
        return { tag: "ok", val: results };
    } catch (error) {
        return { tag: "err", val: `Failed to add observations: ${error.message}` };
    }
}

/**
 * Delete entities from the knowledge graph
 */
function deleteEntities(entityNames) {
    try {
        for (const name of entityNames) {
            // Remove the entity
            const entityIndex = entities.findIndex(e => e.name === name);
            if (entityIndex !== -1) {
                entities.splice(entityIndex, 1);
            }
            
            // Remove all relations involving this entity
            relations = relations.filter(r => r.fromEntity !== name && r.toEntity !== name);
        }
        
        return { tag: "ok", val: null };
    } catch (error) {
        return { tag: "err", val: `Failed to delete entities: ${error.message}` };
    }
}

/**
 * Delete specific observations from entities
 */
function deleteObservations(deletions) {
    try {
        for (const deletion of deletions) {
            const entity = findEntity(deletion.entityName);
            
            if (!entity) {
                return { 
                    tag: "err", 
                    val: `Entity with name ${deletion.entityName} not found` 
                };
            }
            
            // Remove specified observations
            for (const observation of deletion.observations) {
                const index = entity.observations.indexOf(observation);
                if (index !== -1) {
                    entity.observations.splice(index, 1);
                }
            }
        }
        
        return { tag: "ok", val: null };
    } catch (error) {
        return { tag: "err", val: `Failed to delete observations: ${error.message}` };
    }
}

/**
 * Delete relations from the knowledge graph
 */
function deleteRelations(relationsToDelete) {
    try {
        for (const relation of relationsToDelete) {
            const index = relations.findIndex(r => 
                r.fromEntity === relation.fromEntity && 
                r.toEntity === relation.toEntity && 
                r.relationType === relation.relationType
            );
            
            if (index !== -1) {
                relations.splice(index, 1);
            }
        }
        
        return { tag: "ok", val: null };
    } catch (error) {
        return { tag: "err", val: `Failed to delete relations: ${error.message}` };
    }
}

/**
 * Read the entire knowledge graph
 */
function readGraph() {
    try {
        return { 
            tag: "ok", 
            val: {
                entities: entities,
                relations: relations
            }
        };
    } catch (error) {
        return { tag: "err", val: `Failed to read graph: ${error.message}` };
    }
}

/**
 * Search for nodes based on a query string
 */
function searchNodes(query) {
    try {
        const lowerQuery = query.toLowerCase();
        const matchingEntities = [];
        
        // Search through all entities
        for (const entity of entities) {
            let matches = false;
            
            // Check entity name
            if (entity.name.toLowerCase().includes(lowerQuery)) {
                matches = true;
            }
            
            // Check entity type
            if (entity.entityType.toLowerCase().includes(lowerQuery)) {
                matches = true;
            }
            
            // Check observations
            for (const observation of entity.observations) {
                if (observation.toLowerCase().includes(lowerQuery)) {
                    matches = true;
                    break;
                }
            }
            
            if (matches) {
                matchingEntities.push(entity);
            }
        }
        
        // Get entity names for filtering relations
        const entityNames = matchingEntities.map(e => e.name);
        
        // Filter relations that involve matching entities
        const matchingRelations = relations.filter(r => 
            entityNames.includes(r.fromEntity) || entityNames.includes(r.toEntity)
        );
        
        return { 
            tag: "ok", 
            val: {
                entities: matchingEntities,
                relations: matchingRelations
            }
        };
    } catch (error) {
        return { tag: "err", val: `Failed to search nodes: ${error.message}` };
    }
}

/**
 * Open specific nodes by their names
 */
function openNodes(names) {
    try {
        const requestedEntities = [];
        
        // Find all requested entities
        for (const name of names) {
            const entity = findEntity(name);
            if (entity) {
                requestedEntities.push(entity);
            }
        }
        
        // Filter relations that involve requested entities
        const requestedRelations = relations.filter(r => 
            names.includes(r.fromEntity) || names.includes(r.toEntity)
        );
        
        return { 
            tag: "ok", 
            val: {
                entities: requestedEntities,
                relations: requestedRelations
            }
        };
    } catch (error) {
        return { tag: "err", val: `Failed to open nodes: ${error.message}` };
    }
}

// Export the knowledge graph operations interface
export const knowledgeGraphOps = {
    createEntities,
    createRelations,
    addObservations,
    deleteEntities,
    deleteObservations,
    deleteRelations,
    readGraph,
    searchNodes,
    openNodes
};
