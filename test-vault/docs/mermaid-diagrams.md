---
title: "Mermaid Diagram Examples"
tags: [diagrams, mermaid, documentation]
category: documentation
author: "Dev Team"
status: published
---

# Mermaid Diagram Examples

A collection of mermaid diagrams used across the project for visual documentation.

## System Architecture Flowchart

High-level request flow through the platform:

```mermaid
graph TD
    Client[Client App] -->|HTTPS| Gateway[API Gateway]
    Gateway -->|Route| Auth[Auth Service]
    Gateway -->|Route| API[Core API]
    API -->|Query| DB[(PostgreSQL)]
    API -->|Publish| Queue[Message Queue]
    Queue -->|Subscribe| Worker[Background Worker]
    Worker -->|Write| DB
    Worker -->|Notify| Notify[Notification Service]
```

## Sequence Diagram

Authentication flow between services:

```mermaid
sequenceDiagram
    participant U as User
    participant G as Gateway
    participant A as Auth Service
    participant D as Database

    U->>G: POST /login (credentials)
    G->>A: Validate credentials
    A->>D: Query user record
    D-->>A: User data
    A-->>G: JWT token
    G-->>U: 200 OK + token

    U->>G: GET /api/data (Bearer token)
    G->>A: Verify token
    A-->>G: Token valid
    G->>D: Fetch data
    D-->>G: Result set
    G-->>U: 200 OK + data
```

## Class Diagram

Core domain model:

```mermaid
classDiagram
    class Document {
        +String id
        +String title
        +String content
        +DateTime createdAt
        +DateTime updatedAt
        +getChunks() Chunk[]
        +getEmbedding() Vector
    }

    class Chunk {
        +String id
        +String content
        +int tokenCount
        +Vector embedding
    }

    class Collection {
        +String name
        +String rootPath
        +Document[] documents
        +search(query) SearchResult[]
        +ingest() IngestResult
    }

    Collection "1" --> "*" Document : contains
    Document "1" --> "*" Chunk : split into
```

## State Diagram

Document lifecycle in the index:

```mermaid
stateDiagram-v2
    [*] --> Discovered: File scan
    Discovered --> Parsing: Read file
    Parsing --> Chunked: Split by headings
    Chunked --> Embedding: Generate vectors
    Embedding --> Indexed: Store in HNSW
    Indexed --> Stale: File modified
    Stale --> Parsing: Re-ingest
    Indexed --> [*]: File deleted
```

## Entity Relationship Diagram

Index storage model:

```mermaid
erDiagram
    FILE ||--o{ CHUNK : contains
    CHUNK ||--|| VECTOR : "has embedding"
    FILE {
        string path PK
        string hash
        datetime modified_at
        json frontmatter
    }
    CHUNK {
        string id PK
        string file_path FK
        string heading
        string content
        int token_count
    }
    VECTOR {
        string chunk_id FK
        float[] embedding
    }
```

## Gantt Chart

Release timeline:

```mermaid
gantt
    title Release Timeline
    dateFormat YYYY-MM-DD
    section Core
        Foundation & Config    :done, p1, 2025-01-01, 14d
        Markdown Parsing       :done, p2, after p1, 10d
        Chunking Engine        :done, p3, after p2, 7d
        Embedding Providers    :done, p4, after p3, 10d
    section Storage
        Index Storage          :done, p5, after p4, 14d
        Semantic Search        :done, p6, after p5, 10d
    section Polish
        CLI & Library API      :done, p10, after p6, 14d
        Hybrid Search          :done, p14, after p10, 10d
        Mermaid Support        :active, p21, after p14, 7d
```

## Pie Chart

Test distribution by module:

```mermaid
pie title Test Distribution
    "Search" : 89
    "Index" : 72
    "Parser" : 65
    "Chunker" : 48
    "Embedding" : 55
    "CLI" : 42
    "Links" : 68
    "Other" : 173
```

## Journey Map

User experience for search:

```mermaid
journey
    title Search Experience
    section Discovery
        Open collection: 5: User
        Browse file tree: 4: User
        Type search query: 5: User
    section Results
        View ranked results: 4: User, System
        Preview matched chunks: 5: User, System
        Open full document: 5: User
    section Refinement
        Filter by metadata: 3: User
        Expand graph context: 4: User, System
```

## Mindmap

Project structure overview:

```mermaid
mindmap
    root((markdown-vdb))
        Core
            Parser
            Chunker
            Embeddings
        Storage
            HNSW Index
            BM25 FTS
            Link Graph
        Interface
            CLI
            Library API
            Desktop App
        Testing
            Unit Tests
            Integration
            E2E
```

## Invalid Diagram (for error testing)

This block has intentionally broken syntax to verify error handling:

```mermaid
graph TD
    A --> B
    B --> C
    this is not valid mermaid --->>><<<
```
