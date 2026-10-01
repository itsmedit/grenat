//! The fields of a record, as written on the command line: `subject:String`,
//! `score:Float?` (optional), `embedding:Vector(1536)` (an embedding, quoted
//! for the shell: `"embedding:Vector(1536)"`).

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: FieldType,
    pub optional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldType {
    String,
    Int,
    Float,
    Bool,
    /// So many floats: an embedding.
    Vector(u32),
}

/// The largest vector pgvector stores.
const MAX_DIMENSIONS: u32 = 16_000;

impl Field {
    pub fn parse(spec: &str) -> Result<Field, String> {
        let usage = || {
            format!(
                "invalid field `{spec}`: write `name:Type`, Type being String, Int, Float, Bool or Vector(1536) (`Int?`: optional)"
            )
        };
        let (name, ty) = spec.split_once(':').ok_or_else(usage)?;
        if !crate::names::valid(name) {
            return Err(usage());
        }
        if name == "id" {
            return Err("`id` is every record's own: the database gives it".into());
        }
        let (ty, optional) = ty.strip_suffix('?').map_or((ty, false), |t| (t, true));
        let ty = match ty {
            "String" => FieldType::String,
            "Int" => FieldType::Int,
            "Float" => FieldType::Float,
            "Bool" => FieldType::Bool,
            _ => {
                let size = ty.strip_prefix("Vector(").and_then(|t| t.strip_suffix(')')).ok_or_else(usage)?;
                match size.parse::<u32>() {
                    Ok(n) if (1..=MAX_DIMENSIONS).contains(&n) => FieldType::Vector(n),
                    _ => return Err(format!("invalid field `{spec}`: a vector has 1 to {MAX_DIMENSIONS} dimensions")),
                }
            }
        };
        Ok(Field { name: name.to_string(), ty, optional })
    }

    /// Its declaration in a `struct`.
    pub fn declaration(&self) -> String {
        format!("{}: {}{}", self.name, self.type_name(), if self.optional { "?" } else { "" })
    }

    /// Whether it holds an embedding (`nearest` searches it).
    pub fn is_vector(&self) -> bool {
        matches!(self.ty, FieldType::Vector(_))
    }

    /// Its column: SQL that SQLite and PostgreSQL both accept (a vector's
    /// type is the database's: `db.vector(n)`, inside the migration's text).
    pub fn column(&self) -> String {
        let sql = match self.ty {
            FieldType::String => "TEXT".to_string(),
            FieldType::Int => "BIGINT".into(),
            FieldType::Float => "DOUBLE PRECISION".into(),
            FieldType::Bool => "BOOLEAN".into(),
            FieldType::Vector(n) => format!("#{{db.vector({n})}}"),
        };
        format!("{} {sql}{}", self.name, if self.optional { "" } else { " NOT NULL" })
    }

    /// A value for tests.
    pub fn sample(&self) -> String {
        match self.ty {
            FieldType::String => format!("\"{}\"", self.name),
            FieldType::Int => "1".into(),
            FieldType::Float => "1.5".into(),
            FieldType::Bool => "true".into(),
            FieldType::Vector(n) => format!("(1..{n}).map {{ |i| i.to_f }}"),
        }
    }

    fn type_name(&self) -> String {
        match self.ty {
            FieldType::String => "String".into(),
            FieldType::Int => "Int".into(),
            FieldType::Float => "Float".into(),
            FieldType::Bool => "Bool".into(),
            FieldType::Vector(n) => format!("Vector({n})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields() {
        let score = Field::parse("score:Float?").unwrap();
        assert_eq!(score.declaration(), "score: Float?");
        assert_eq!(score.column(), "score DOUBLE PRECISION");
        let subject = Field::parse("subject:String").unwrap();
        assert_eq!(subject.column(), "subject TEXT NOT NULL");
        assert_eq!(subject.sample(), "\"subject\"");
        assert!(Field::parse("id:Int").unwrap_err().contains("database gives it"));
        for bad in ["subject", "subject:Text", "Subject:String", ":Int", "e:Vector", "e:Vector()"] {
            assert!(Field::parse(bad).is_err(), "{bad}");
        }
        let embedding = Field::parse("embedding:Vector(1536)").unwrap();
        assert_eq!(embedding.declaration(), "embedding: Vector(1536)");
        assert_eq!(embedding.column(), "embedding #{db.vector(1536)} NOT NULL");
        assert_eq!(embedding.sample(), "(1..1536).map { |i| i.to_f }");
        assert!(embedding.is_vector());
        assert_eq!(Field::parse("e:Vector(8)?").unwrap().column(), "e #{db.vector(8)}");
        for bad in ["e:Vector(0)", "e:Vector(16001)", "e:Vector(x)"] {
            assert!(Field::parse(bad).unwrap_err().contains("1 to 16000 dimensions"), "{bad}");
        }
    }
}
